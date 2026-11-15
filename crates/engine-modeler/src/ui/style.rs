//! Цвета и мелкие виджеты окна моделей.
//!
//! Цвета берутся из текущего стиля egui: в редакторе и в отдельной программе это общая
//! тема Piska (её ставит хозяин окна), поэтому смена темы меняет и окно моделей.

use egui::{pos2, vec2, Color32, Rounding, FontId, Rect, Response, Sense, Stroke, Ui};

use engine_i18n::{tr, trf};

use super::icons::{self, Icon};

/// Палитра интерфейса на этот кадр.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub bg_app: Color32,
    pub bg_panel: Color32,
    pub bg_raised: Color32,
    pub bg_field: Color32,
    pub bg_inset: Color32,
    pub hover: Color32,
    pub active: Color32,
    pub line: Color32,
    pub line_strong: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub text_faint: Color32,
    pub accent: Color32,
    pub axis: [Color32; 3],
    pub warn: Color32,
    pub error: Color32,
}

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

impl Palette {
    pub fn from_ctx(ctx: &egui::Context) -> Self {
        let v = ctx.style().visuals.clone();
        let text = v.widgets.noninteractive.fg_stroke.color;
        let panel = v.panel_fill;
        Self {
            bg_app: mix(panel, Color32::BLACK, 0.35),
            bg_panel: panel,
            bg_raised: v.faint_bg_color,
            bg_field: v.widgets.inactive.bg_fill,
            bg_inset: v.extreme_bg_color,
            hover: v.widgets.hovered.bg_fill,
            active: v.widgets.active.bg_fill,
            line: v.widgets.noninteractive.bg_stroke.color,
            line_strong: v.window_stroke.color,
            text,
            text_muted: mix(text, panel, 0.33),
            text_faint: mix(text, panel, 0.58),
            accent: v.hyperlink_color,
            axis: [
                Color32::from_rgb(0xe8, 0x55, 0x55),
                Color32::from_rgb(0x74, 0xc9, 0x52),
                Color32::from_rgb(0x4f, 0x92, 0xec),
            ],
            warn: v.warn_fg_color,
            error: v.error_fg_color,
        }
    }

    pub fn accent_soft(&self) -> Color32 {
        mix(self.bg_panel, self.accent, 0.28)
    }
}

pub fn small() -> FontId {
    FontId::proportional(11.5)
}

pub fn body() -> FontId {
    FontId::proportional(13.0)
}

/// Квадратная кнопка с иконкой. `id_name` — стабильный идентификатор (тесты щёлкают по нему).
pub fn icon_button(ui: &mut Ui, id_name: &str, icon: Icon, selected: bool, enabled: bool, tooltip: &str) -> Response {
    let p = Palette::from_ctx(ui.ctx());
    // Высота любого контрола строки — CONTROL_H (как кнопки панелей редактора).
    let size = vec2(CONTROL_H, CONTROL_H);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let id = egui::Id::new(id_name);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let response = ui.interact(rect, id, sense);
    let bg = if selected {
        p.accent_soft()
    } else if response.is_pointer_button_down_on() && enabled {
        p.active
    } else if response.hovered() && enabled {
        p.hover
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, Rounding::same(super::kit::R_S), bg);
    if selected {
        // Рамка — внутри кнопки.
        ui.painter()
            .rect_stroke(rect.shrink(0.5), Rounding::same(super::kit::R_S), Stroke::new(1.0_f32, mix(p.accent, p.bg_panel, 0.35)));
    }
    let color = if !enabled {
        p.text_faint
    } else if selected {
        Color32::WHITE
    } else if response.hovered() {
        p.text
    } else {
        p.text_muted
    };
    icons::paint(ui.painter(), rect.shrink(((CONTROL_H - 16.0) * 0.5).max(2.0)), icon, color);
    // Простая подсказка переводится здесь; составную (`tip`) переводит сам `tip`, а её
    // готовый текст в каталоге не найдётся и остаётся как есть.
    response.on_hover_text(tr(tooltip))
}

/// Заголовок сворачиваемого раздела правой колонки. Возвращает, открыт ли раздел.
pub fn section(ui: &mut Ui, title: &str, open: &mut bool, right: impl FnOnce(&mut Ui)) -> bool {
    let p = Palette::from_ctx(ui.ctx());
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 26.0), Sense::hover());
    let id = egui::Id::new(("modeler.section", title));
    let header = ui.interact(rect.with_max_x(rect.max.x - 64.0), id, Sense::click());
    if header.clicked() {
        *open = !*open;
    }
    ui.painter().rect_filled(rect, Rounding::ZERO, p.bg_raised);
    ui.painter().line_segment(
        [pos2(rect.min.x, rect.max.y - 0.5), pos2(rect.max.x, rect.max.y - 0.5)],
        Stroke::new(1.0_f32, p.line),
    );
    let chevron = Rect::from_center_size(pos2(rect.min.x + 12.0, rect.center().y), vec2(9.0, 9.0));
    icons::paint(
        ui.painter(),
        chevron,
        if *open { Icon::ChevronDown } else { Icon::ChevronRight },
        p.text_muted,
    );
    ui.painter().text(
        pos2(rect.min.x + 22.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        tr(title).to_uppercase(),
        FontId::proportional(11.0),
        if header.hovered() { p.text } else { p.text_muted },
    );
    let mut right_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(vec2(6.0, 2.0)))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    right(&mut right_ui);
    *open
}

/// Строка «подпись — содержимое» с подписью фиксированной ширины.
pub fn row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    let p = Palette::from_ctx(ui.ctx());
    ui.horizontal(|ui| {
        // Колонка подписей — одна для всех строк панели (28 % ширины, 62…96 px: полям рядом нужно
        // место); длинная
        // подпись обрезается «…» с подсказкой и не налезает на поле рядом.
        let column = (ui.available_width() * 0.28).clamp(62.0, 96.0).floor();
        let (rect, response) = ui.allocate_exact_size(vec2(column, 22.0), Sense::hover());
        let text = tr(label);
        let mut job = egui::text::LayoutJob::single_section(
            text.to_string(),
            egui::text::TextFormat { font_id: small(), color: p.text_muted, ..Default::default() },
        );
        job.wrap = egui::text::TextWrapping { max_width: column - 4.0, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
        let galley = ui.painter().layout_job(job);
        let cut = galley.size().x < ui.painter().layout_no_wrap(text.to_string(), small(), p.text_muted).size().x - 0.5;
        ui.painter().galley(pos2(rect.min.x, rect.center().y - galley.size().y * 0.5), galley, p.text_muted);
        if cut {
            response.on_hover_text(text.to_string());
        }
        add(ui);
    });
}

/// Три поля X / Y / Z с цветными метками. Возвращает, изменилось ли значение, и было ли
/// это началом / концом перетаскивания (для истории).
pub fn vec3_fields(ui: &mut Ui, id: &str, value: &mut [f32; 3], speed: f64, suffix: &str) -> FieldEdit {
    let p = Palette::from_ctx(ui.ctx());
    let mut edit = FieldEdit::default();
    // Три метки по 12 и пять промежутков между шестью элементами — поля делят остаток.
    let gaps = ui.spacing().item_spacing.x * 5.0;
    let width = ((ui.available_width() - 36.0 - gaps) / 3.0 - 0.5).floor().max(30.0);
    for axis in 0..3 {
        let (tag, _) = ui.allocate_exact_size(vec2(12.0, 20.0), Sense::hover());
        ui.painter().text(
            tag.center(),
            egui::Align2::CENTER_CENTER,
            ["X", "Y", "Z"][axis],
            FontId::proportional(10.5),
            p.axis[axis],
        );
        let response = super::controls::number_field_ex(ui, &mut value[axis], f32::MIN..=f32::MAX, speed, "", suffix, 3, width);
        let response = response.on_hover_text(trf!("{field} {axis}: drag or type", field = tr(id), axis = ["X", "Y", "Z"][axis]));
        edit.merge(&response);
    }
    edit
}

/// Что произошло с полем ввода в этом кадре.
#[derive(Clone, Copy, Debug, Default)]
pub struct FieldEdit {
    pub changed: bool,
    pub started: bool,
    pub finished: bool,
}

impl FieldEdit {
    pub fn merge(&mut self, r: &Response) {
        self.changed |= r.changed();
        self.started |= r.drag_started() || r.gained_focus();
        self.finished |= r.drag_stopped() || r.lost_focus();
    }
}

/// Цветной образец (палитра). Возвращает ответ для щелчка.
pub fn swatch(ui: &mut Ui, id: egui::Id, color: Color32, size: f32, selected: bool) -> Response {
    let p = Palette::from_ctx(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let response = ui.interact(rect, id, Sense::click());
    checker(ui.painter(), rect, 4.0);
    ui.painter().rect_filled(rect, Rounding::same(2.0), color);
    let stroke = if selected {
        Stroke::new(2.0_f32, Color32::WHITE)
    } else if response.hovered() {
        Stroke::new(1.0_f32, p.text)
    } else {
        Stroke::new(1.0_f32, mix(p.line_strong, Color32::BLACK, 0.3))
    };
    // Обводка — внутри образца (не вылезает за его прямоугольник).
    ui.painter().rect_stroke(rect.shrink(stroke.width * 0.5), Rounding::same(2.0), stroke);
    response
}

/// Крупный образец текущего цвета (без щелчка — только показ и подсказка).
pub fn color_preview(ui: &mut Ui, color: Color32, size: f32) -> Response {
    let p = Palette::from_ctx(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    checker(ui.painter(), rect, 4.0);
    ui.painter().rect_filled(rect, Rounding::same(2.0), color);
    ui.painter().rect_stroke(rect.shrink(0.5), Rounding::same(2.0), Stroke::new(1.0_f32, mix(p.line_strong, Color32::BLACK, 0.3)));
    response
}

/// Шахматка под прозрачным цветом.
pub fn checker(painter: &egui::Painter, rect: Rect, cell: f32) {
    painter.rect_filled(rect, Rounding::ZERO, Color32::from_gray(0x9a));
    let mut y = rect.min.y;
    let mut row = 0;
    while y < rect.max.y {
        let mut x = rect.min.x + if row % 2 == 0 { 0.0 } else { cell };
        while x < rect.max.x {
            let r = Rect::from_min_max(pos2(x, y), pos2((x + cell).min(rect.max.x), (y + cell).min(rect.max.y)));
            painter.rect_filled(r, Rounding::ZERO, Color32::from_gray(0x6e));
            x += cell * 2.0;
        }
        y += cell;
        row += 1;
    }
}

pub fn to_color32(c: crate::image::Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
}

pub fn from_color32(c: Color32) -> crate::image::Rgba {
    c.to_srgba_unmultiplied()
}

/// Подпись-подсказка горячей клавиши справа в тексте подсказки: `"Move  W"`.
pub fn tip(name: &str, key: &str, what: &str) -> String {
    let (name, what) = (tr(name), tr(what));
    if key.is_empty() {
        format!("{name}\n{what}")
    } else {
        format!("{name}  ·  {key}\n{what}")
    }
}

/// Название стороны для показа: «North» → «Север». Английское имя остаётся ключом в файлах.
// i18n-ctx: face "North" "East" "South" "West" "Up" "Down"
pub fn face_name(dir: crate::doc::FaceDir) -> std::borrow::Cow<'static, str> {
    engine_i18n::tr_ctx("face", dir.name())
}

/// Название формы для показа: «Cuboid» → «Кубоид».
pub fn shape_name(shape: &crate::doc::Shape) -> std::borrow::Cow<'static, str> {
    engine_i18n::tr(shape.name())
}

// ---------------------------------------------------------------------------
// Полосы заголовка: группы настроек инструмента
// ---------------------------------------------------------------------------

/// Высота элемента полосы (шкала темы).
pub const CONTROL_H: f32 = super::kit::CONTROL_H;

/// Волосяная вертикальная линия между группами полосы, на всю высоту элемента.
pub fn hairline(ui: &mut Ui) {
    let p = Palette::from_ctx(ui.ctx());
    ui.add_space(super::kit::SP_S);
    let (rect, _) = ui.allocate_exact_size(vec2(1.0, CONTROL_H), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, p.line_strong);
    ui.add_space(super::kit::SP_S);
}

/// Подпись группы: мелко и приглушённо, по базовой линии с элементами.
pub fn caption(ui: &mut Ui, text: &str) {
    let p = Palette::from_ctx(ui.ctx());
    ui.add(egui::Label::new(egui::RichText::new(text).size(super::kit::FONT_CAPTION).color(p.text_muted)).selectable(false));
}

/// Сегментированный переключатель: варианты одним блоком, выбранный — с акцентом.
/// Возвращает выбранное значение, если щёлкнули другой вариант.
pub fn segmented<T: Copy + PartialEq>(ui: &mut Ui, id: &str, value: T, options: &[(T, Option<Icon>, &str, &str)]) -> Option<T> {
    let p = Palette::from_ctx(ui.ctx());
    let mut picked = None;
    let font = FontId::proportional(super::kit::FONT_BODY);
    let widths: Vec<f32> = options
        .iter()
        .map(|(_, icon, label, _)| {
            let text = if label.is_empty() {
                0.0
            } else {
                ui.painter().layout_no_wrap(label.to_string(), font.clone(), p.text).size().x
            };
            let icon_w = if icon.is_some() { 16.0 + if label.is_empty() { 0.0 } else { 4.0 } } else { 0.0 };
            (text + icon_w + 2.0 * super::kit::SP_M).max(CONTROL_H)
        })
        .collect();
    let total: f32 = widths.iter().sum();
    let (outer, _) = ui.allocate_exact_size(vec2(total, CONTROL_H), Sense::hover());
    ui.painter().rect_filled(outer, Rounding::same(super::kit::R_M), p.bg_field);
    let mut x = outer.min.x;
    for (i, ((option, icon, label, tooltip), w)) in options.iter().zip(&widths).enumerate() {
        let rect = Rect::from_min_size(pos2(x, outer.min.y), vec2(*w, CONTROL_H));
        x += w;
        let response = ui.interact(rect, egui::Id::new((id, i)), Sense::click());
        let selected = *option == value;
        if selected {
            ui.painter().rect_filled(rect.shrink(1.0), Rounding::same(super::kit::R_S), p.accent_soft());
        } else if response.hovered() {
            ui.painter().rect_filled(rect.shrink(1.0), Rounding::same(super::kit::R_S), p.hover);
        }
        if i > 0 && !selected && options[i - 1].0 != value {
            ui.painter().line_segment(
                [pos2(rect.min.x, rect.min.y + 4.0), pos2(rect.min.x, rect.max.y - 4.0)],
                Stroke::new(1.0_f32, p.line_strong),
            );
        }
        let color = if selected { Color32::WHITE } else if response.hovered() { p.text } else { p.text_muted };
        let mut cx = rect.min.x + super::kit::SP_M;
        if let Some(icon) = icon {
            let r = if label.is_empty() {
                Rect::from_center_size(rect.center(), vec2(14.0, 14.0))
            } else {
                Rect::from_min_size(pos2(cx, rect.center().y - 7.0), vec2(14.0, 14.0))
            };
            icons::paint(ui.painter(), r, *icon, color);
            cx += 20.0;
        }
        if !label.is_empty() {
            ui.painter().text(pos2(cx, rect.center().y), egui::Align2::LEFT_CENTER, label, font.clone(), color);
        }
        if response.on_hover_text(*tooltip).clicked() && !selected {
            picked = Some(*option);
        }
    }
    picked
}

/// Число со шагом: [−] значение [+]; колесо и перетаскивание по полю тоже меняют.
pub fn stepper(ui: &mut Ui, id: &str, value: &mut u32, range: std::ops::RangeInclusive<u32>, suffix: &str, tooltip: &str) -> bool {
    let mut changed = false;
    ui.spacing_mut().item_spacing.x = 0.0;
    let small_button = |ui: &mut Ui, label: &str, salt: &str| {
        ui.push_id((id, salt), |ui| {
            super::controls::button_ex(ui, super::controls::Kind::Normal, None, label, CONTROL_H)
        })
        .inner
    };
    if small_button(ui, "−", "minus").on_hover_text(tooltip).clicked() && *value > *range.start() {
        *value -= 1;
        changed = true;
    }
    let field = super::controls::number_field_ex(ui, value, range.clone(), 0.1, "", suffix, 0, 44.0);
    changed |= field.on_hover_text(tooltip).changed();
    if small_button(ui, "+", "plus").on_hover_text(tooltip).clicked() && *value < *range.end() {
        *value += 1;
        changed = true;
    }
    ui.spacing_mut().item_spacing.x = super::kit::SP_S;
    changed
}

/// Переключатель с иконкой и подписью (вкл. — с акцентом).
pub fn toggle(ui: &mut Ui, id: &str, icon: Icon, label: &str, on: bool, tooltip: &str) -> Response {
    let p = Palette::from_ctx(ui.ctx());
    let font = FontId::proportional(super::kit::FONT_BODY);
    let text_w = ui.painter().layout_no_wrap(label.to_string(), font.clone(), p.text).size().x;
    let (rect, _) = ui.allocate_exact_size(vec2(text_w + 16.0 + 4.0 + 2.0 * super::kit::SP_M, CONTROL_H), Sense::hover());
    let response = ui.interact(rect, egui::Id::new(id), Sense::click());
    let bg = if on { p.accent_soft() } else if response.hovered() { p.hover } else { p.bg_field };
    ui.painter().rect_filled(rect, Rounding::same(super::kit::R_M), bg);
    let color = if on { Color32::WHITE } else if response.hovered() { p.text } else { p.text_muted };
    icons::paint(
        ui.painter(),
        Rect::from_min_size(pos2(rect.min.x + super::kit::SP_M, rect.center().y - 7.0), vec2(14.0, 14.0)),
        icon,
        color,
    );
    ui.painter().text(
        pos2(rect.min.x + super::kit::SP_M + 20.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        color,
    );
    response.on_hover_text(tooltip)
}

/// Значок и имя активного инструмента — первым в полосе настроек.
pub fn tool_badge(ui: &mut Ui, icon: Icon, name: &str) {
    let p = Palette::from_ctx(ui.ctx());
    let font = FontId::new(super::kit::FONT_BODY, egui::FontFamily::Proportional);
    let text_w = ui.painter().layout_no_wrap(name.to_string(), font.clone(), p.text).size().x;
    let (rect, _) = ui.allocate_exact_size(vec2(16.0 + 6.0 + text_w, CONTROL_H), Sense::hover());
    icons::paint(ui.painter(), Rect::from_min_size(pos2(rect.min.x, rect.center().y - 8.0), vec2(16.0, 16.0)), icon, p.accent);
    ui.painter().text(pos2(rect.min.x + 22.0, rect.center().y), egui::Align2::LEFT_CENTER, name, font, p.text);
}
