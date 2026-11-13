//! КОПИЯ визуальной системы движка Piska — не править руками.
//!
//! Источник: `crates/engine-editor/src/theme.rs` движка (палитра, шкалы, шрифты, `apply`).
//! Обновить: `python scripts/sync_from_engine.py --engine <движок>`. Так Piska Modeler
//! выглядит как редактор движка и собирается без него.
#![allow(dead_code, unused_imports)]

// Визуальная система редактора: палитра, шкалы отступов, шрифтов и скруглений,
// и небольшие виджеты, из которых собраны все панели (вкладки, заголовки, пункты
// меню, кнопки диалогов, поля поиска).
//
// Правило одно: цвета, размеры и отступы берутся отсюда, а не пишутся числами в панелях.
// Палитра — сдержанная тёмная с одним акцентом; акцент выбирается в Preferences.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use egui::text::{LayoutJob, TextFormat};
use egui::{pos2, vec2, Align2, Color32, FontFamily, FontId, Rect, Response, Rounding, Sense, Stroke, TextStyle, Ui};


// ---------------------------------------------------------------------------
// Палитра
// ---------------------------------------------------------------------------

// Нейтральные серые без синевы; уровни близко друг к другу (docs/design/ui-guidelines.md).

/// Фон окна под панелями, строка состояния; им же видна линия между панелями.
pub const BG_APP: Color32 = Color32::from_rgb(0x18, 0x18, 0x18);
/// Полосы вкладок, меню, главная панель инструментов.
pub const BG_HEADER: Color32 = Color32::from_rgb(0x1f, 0x1f, 0x1f);
/// Содержимое панелей.
pub const BG_PANEL: Color32 = Color32::from_rgb(0x26, 0x26, 0x26);
/// Поднятое над панелью: заголовки компонентов, меню, окна, подсказки.
pub const BG_RAISED: Color32 = Color32::from_rgb(0x2c, 0x2c, 0x2c);
/// Кнопки и выпадающие списки.
pub const BG_FIELD: Color32 = Color32::from_rgb(0x35, 0x35, 0x35);
/// Утопленные поля ввода (текст, числа, поиск).
pub const BG_INSET: Color32 = Color32::from_rgb(0x1c, 0x1c, 0x1c);
pub const BG_HOVER: Color32 = Color32::from_rgb(0x3e, 0x3e, 0x3e);
pub const BG_ACTIVE: Color32 = Color32::from_rgb(0x48, 0x48, 0x48);
/// Волосяные линии: между панелями, под полосами, между строками таблиц.
pub const LINE: Color32 = Color32::from_rgb(0x14, 0x14, 0x14);
/// Рамки полей и кнопок.
pub const LINE_STRONG: Color32 = Color32::from_rgb(0x0f, 0x0f, 0x0f);
/// Светлая линия на поднятых поверхностях (разделители меню, края заголовков).
pub const LINE_SOFT: Color32 = Color32::from_rgb(0x30, 0x30, 0x30);
/// Рамка поля под курсором.
pub const LINE_HOVER: Color32 = Color32::from_rgb(0x55, 0x55, 0x55);

pub const TEXT: Color32 = Color32::from_rgb(0xdc, 0xdc, 0xdc);
/// Текст на акценте, выбранная вкладка, имя под курсором.
pub const TEXT_STRONG: Color32 = Color32::from_rgb(0xf2, 0xf2, 0xf2);
/// Вторичный текст: подписи полей, пути, счётчики. Читается, а не «угадывается».
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0xa0, 0xa0, 0xa0);
/// Недоступное.
pub const TEXT_DISABLED: Color32 = Color32::from_rgb(0x6a, 0x6a, 0x6a);

pub const OK: Color32 = Color32::from_rgb(0x6a, 0xa8, 0x74);
pub const WARN: Color32 = Color32::from_rgb(0xd2, 0xa2, 0x4c);
pub const ERROR: Color32 = Color32::from_rgb(0xd6, 0x5f, 0x5a);
/// Информационные сообщения.
pub const INFO: Color32 = Color32::from_rgb(0x6f, 0x9f, 0xd2);
/// Экземпляры префабов — как в Unity, голубым.
pub const PREFAB: Color32 = Color32::from_rgb(0x6a, 0xb0, 0xe0);
/// Каталоги в окне Project — приглушённый жёлтый.
pub const FOLDER: Color32 = Color32::from_rgb(0xc8, 0xa8, 0x5a);
pub const AXIS_X: Color32 = Color32::from_rgb(0xe0, 0x52, 0x4a);
pub const AXIS_Y: Color32 = Color32::from_rgb(0x7c, 0xc0, 0x4e);
pub const AXIS_Z: Color32 = Color32::from_rgb(0x4a, 0x8c, 0xe6);

/// Фон вида Scene (линейное пространство — его получает рендерер).
pub const SCENE_CLEAR: [f32; 3] = [0.016, 0.016, 0.016];

/// Готовые акцентные цвета (Preferences → Accent). Все приглушённые: акцент отмечает
/// выделение и фокус, а не украшает.
pub const ACCENTS: [(&str, Color32); 5] = [
    ("Blue", Color32::from_rgb(0x47, 0x72, 0xb3)),
    ("Orange", Color32::from_rgb(0xc4, 0x75, 0x30)),
    ("Green", Color32::from_rgb(0x46, 0x8f, 0x5e)),
    ("Purple", Color32::from_rgb(0x7d, 0x66, 0xb8)),
    ("Teal", Color32::from_rgb(0x34, 0x8e, 0x96)),
];

static ACCENT: AtomicU32 = AtomicU32::new(0x4772b3);

/// Акцент: выделение, активная вкладка, главная кнопка, режим Play.
pub fn accent() -> Color32 {
    let rgb = ACCENT.load(Ordering::Relaxed);
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

pub fn set_accent(color: Color32) {
    let rgb = (u32::from(color.r()) << 16) | (u32::from(color.g()) << 8) | u32::from(color.b());
    ACCENT.store(rgb, Ordering::Relaxed);
}

/// Смешать два цвета: `t = 0` — `a`, `t = 1` — `b`.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let lerp = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(lerp(a.r(), b.r()), lerp(a.g(), b.g()), lerp(a.b(), b.b()))
}

/// Фон выделенной строки (непрозрачный: под ним бывают линии-направляющие).
pub fn selection_fill() -> Color32 {
    mix(BG_PANEL, accent(), 0.62)
}

/// Фон выделенной строки, когда окно не в фокусе (как в Unity — серый).
pub fn selection_unfocused() -> Color32 {
    mix(BG_PANEL, Color32::WHITE, 0.13)
}

/// Фон строки под курсором.
pub fn hover_fill() -> Color32 {
    mix(BG_PANEL, Color32::WHITE, 0.045)
}

/// Приглушённый акцент: включённые переключатели, подсветка цели перетаскивания.
pub fn accent_soft() -> Color32 {
    mix(BG_FIELD, accent(), 0.5)
}

/// Тонировка главной панели инструментов в режиме Play (как в Unity; сила — 0…1).
pub fn play_tint(strength: f32) -> Color32 {
    mix(BG_HEADER, accent(), 0.3 * strength.clamp(0.0, 1.0))
}

// ---------------------------------------------------------------------------
// Шкалы
// ---------------------------------------------------------------------------

/// Отступы: 2 · 4 · 8 · 12 · 16 · 24.
pub const SP_XS: f32 = 2.0;
pub const SP_S: f32 = 4.0;
pub const SP_M: f32 = 8.0;
pub const SP_L: f32 = 12.0;
pub const SP_XL: f32 = 16.0;
pub const SP_XXL: f32 = 24.0;

/// Скругления: контролы · плашки и миниатюры · меню и окна. У панелей — 0.
pub const R_S: f32 = 2.0;
pub const R_M: f32 = 3.0;
pub const R_L: f32 = 4.0;

/// Шрифты: подпись · основной · заголовок раздела · заголовок диалога · моно.
pub const FONT_CAPTION: f32 = 11.5;
pub const FONT_BODY: f32 = 12.5;
pub const FONT_TITLE: f32 = 12.5;
pub const FONT_HEADING: f32 = 14.0;
pub const FONT_MONO: f32 = 12.0;
/// Подписи групп прописными (`section_label`).
pub const FONT_SECTION: f32 = 10.5;

/// Высота строки списков (иерархия, проект, консоль, таблицы).
pub const ROW: f32 = 22.0;
/// Высота полосы вкладок дока.
pub const TAB_BAR: f32 = 24.0;
/// Полоса инструментов под вкладкой (`toolbar_strip`, `panel_header`).
pub const STRIP: f32 = 28.0;
/// Поле или кнопка в инспекторе и диалогах.
pub const FIELD_H: f32 = 20.0;
/// Кнопка, поиск, переключатель на полосе инструментов.
pub const CONTROL_H: f32 = 22.0;
/// Главное меню, главная панель инструментов, строка состояния.
pub const MENU_BAR: f32 = 22.0;
pub const MAIN_TOOLBAR: f32 = 32.0;
pub const STATUS_BAR: f32 = 22.0;
/// Иконка: обычная и маленькая (в полях, строке состояния).
pub const ICON: f32 = 16.0;
pub const ICON_S: f32 = 12.0;
/// Шаг вложенности дерева.
pub const INDENT: f32 = 14.0;
/// Высота пункта меню.
pub const MENU_ROW: f32 = 22.0;
/// Минимальная ширина выпадающего меню.
pub const MENU_WIDTH: f32 = 220.0;
/// Колонка иконки в меню.
const MENU_GUTTER: f32 = 26.0;

/// Ширина колонки подписей в инспекторе для панели шириной `width`.
pub fn label_width(width: f32) -> f32 {
    (width * 0.4).clamp(PROP_LABEL_MIN, PROP_LABEL_MAX).min(width * 0.5)
}

/// Сетка свойств (инспектор, редактор материала, страницы настроек): колонка подписей —
/// 40 % ширины в этих пределах…
pub const PROP_LABEL_MIN: f32 = 90.0;
pub const PROP_LABEL_MAX: f32 = 170.0;
/// …колонка значения — остальное, но не шире этого: в широком окне поля стоят слева, а не
/// растягиваются на весь экран. Векторы делят колонку поровну.
pub const PROP_VALUE_MAX: f32 = 340.0;


/// Виджет в области ровно `size`: занимает её в родителе, а всё, что внутри шире или
/// выше, обрезается и не раздвигает окно (длинные имена, тексты, списки).
pub fn fixed<R>(ui: &mut Ui, size: egui::Vec2, add: impl FnOnce(&mut Ui) -> R) -> R {
    let (rect, _) = ui.allocate_exact_size(size.max(egui::Vec2::ZERO), Sense::hover());
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    add(&mut child)
}

/// Колонки строки свойства шириной `width`: (подпись, значение). Сумма с промежутком
/// никогда не больше `width`.
pub fn property_columns(width: f32) -> (f32, f32) {
    let width = width.max(0.0);
    let label = label_width(width);
    let value = (width - label - SP_S).clamp(0.0, PROP_VALUE_MAX);
    (label, value)
}

pub fn body() -> FontId {
    FontId::proportional(FONT_BODY)
}

pub fn caption() -> FontId {
    FontId::proportional(FONT_CAPTION)
}

pub fn mono() -> FontId {
    FontId::monospace(FONT_MONO)
}

/// Полужирный — для заголовков (если в системе нет жирного шрифта, обычный).
pub fn strong(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(STRONG.into()))
}

const STRONG: &str = "strong";

// ---------------------------------------------------------------------------
// Шрифты
// ---------------------------------------------------------------------------

struct SystemFonts {
    regular: Option<Vec<u8>>,
    bold: Option<Vec<u8>>,
    mono: Option<Vec<u8>>,
}

/// Системные шрифты интерфейса (Segoe UI на Windows, DejaVu / Noto на Linux):
/// встроенный в egui шрифт тонкий и мелкий. Чего нет — заменяется встроенным.
fn system_fonts() -> &'static SystemFonts {
    static FONTS: OnceLock<SystemFonts> = OnceLock::new();
    FONTS.get_or_init(|| {
        let windows = std::env::var("WINDIR").map(|dir| format!("{dir}/Fonts/")).unwrap_or_default();
        let first = |candidates: &[String]| candidates.iter().find_map(|path| std::fs::read(path).ok());
        let win = |name: &str| format!("{windows}{name}");
        SystemFonts {
            regular: first(&[
                win("segoeui.ttf"),
                "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf".to_string(),
                "/usr/share/fonts/noto/NotoSans-Regular.ttf".to_string(),
                "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf".to_string(),
                "/usr/share/fonts/TTF/DejaVuSans.ttf".to_string(),
            ]),
            bold: first(&[
                win("segoeuisb.ttf"),
                win("segoeuib.ttf"),
                "/usr/share/fonts/truetype/noto/NotoSans-SemiBold.ttf".to_string(),
                "/usr/share/fonts/truetype/noto/NotoSans-Bold.ttf".to_string(),
                "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf".to_string(),
                "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf".to_string(),
            ]),
            mono: first(&[
                win("CascadiaMono.ttf"),
                win("consola.ttf"),
                "/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-Regular.ttf".to_string(),
                "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf".to_string(),
                "/usr/share/fonts/TTF/DejaVuSansMono.ttf".to_string(),
            ]),
        }
    })
}

fn install_fonts(ctx: &egui::Context) {
    let system = system_fonts();
    let mut fonts = egui::FontDefinitions::default();
    let defaults_proportional = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let defaults_mono = fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let mut add = |name: &str, bytes: &Option<Vec<u8>>| -> Option<String> {
        let bytes = bytes.as_ref()?;
        fonts
            .font_data
            .insert(name.to_string(), Arc::new(egui::FontData::from_owned(bytes.clone())));
        Some(name.to_string())
    };
    let regular = add("ui-regular", &system.regular);
    let bold = add("ui-bold", &system.bold);
    let mono = add("ui-mono", &system.mono);

    let mut proportional: Vec<String> = regular.iter().cloned().collect();
    proportional.extend(defaults_proportional.iter().cloned());
    let mut strong_family: Vec<String> = bold.or(regular).into_iter().collect();
    strong_family.extend(defaults_proportional);
    let mut monospace: Vec<String> = mono.into_iter().collect();
    monospace.extend(defaults_mono);

    fonts.families.insert(FontFamily::Proportional, proportional);
    fonts.families.insert(FontFamily::Monospace, monospace);
    fonts.families.insert(FontFamily::Name(STRONG.into()), strong_family);
    ctx.set_fonts(fonts);
}

// ---------------------------------------------------------------------------
// Стиль egui
// ---------------------------------------------------------------------------

/// Применить тему к контексту (шрифты ставятся один раз; цвета — при каждом вызове,
/// чтобы смена акцента в Preferences сразу была видна).
pub fn apply(ctx: &egui::Context, with_fonts: bool) {
    if with_fonts {
        install_fonts(ctx);
    }
    let accent = accent();
    let mut style = (*ctx.style()).clone();

    style.text_styles = [
        (TextStyle::Small, FontId::proportional(FONT_CAPTION)),
        (TextStyle::Body, FontId::proportional(FONT_BODY)),
        (TextStyle::Button, FontId::proportional(FONT_BODY)),
        (TextStyle::Heading, strong(FONT_HEADING)),
        (TextStyle::Monospace, FontId::monospace(FONT_MONO)),
    ]
    .into();

    let spacing = &mut style.spacing;
    spacing.item_spacing = vec2(SP_S + 2.0, SP_S);
    spacing.button_padding = vec2(SP_M, 2.0);
    spacing.interact_size = vec2(36.0, FIELD_H);
    spacing.indent = INDENT;
    spacing.menu_margin = egui::Margin::same(SP_S);
    spacing.window_margin = egui::Margin::same(SP_L);
    spacing.combo_height = 320.0;
    spacing.menu_width = MENU_WIDTH + 8.0;
    spacing.icon_width = 13.0;
    spacing.icon_width_inner = 7.0;
    spacing.icon_spacing = SP_S + 1.0;
    spacing.tooltip_width = 340.0;
    spacing.slider_rail_height = 4.0;
    spacing.scroll = egui::style::ScrollStyle {
        bar_width: 6.0,
        floating: true,
        floating_width: 2.0,
        floating_allocated_width: 0.0,
        bar_inner_margin: 2.0,
        bar_outer_margin: 1.0,
        handle_min_length: 20.0,
        ..egui::style::ScrollStyle::floating()
    };
    style.interaction.tooltip_delay = 0.4;
    // Числа в полях — моноширинным, как в Unity и Blender.
    style.drag_value_text_style = TextStyle::Monospace;
    style.interaction.show_tooltips_only_when_still = false;

    let visuals = &mut style.visuals;
    *visuals = egui::Visuals::dark();
    visuals.override_text_color = None;
    visuals.panel_fill = BG_PANEL;
    visuals.window_fill = BG_RAISED;
    visuals.extreme_bg_color = BG_INSET;
    visuals.code_bg_color = BG_INSET;
    visuals.faint_bg_color = BG_RAISED;
    visuals.window_stroke = Stroke::new(1.0_f32, LINE_STRONG);
    visuals.window_rounding = Rounding::same(R_L);
    visuals.menu_rounding = Rounding::same(R_L);
    visuals.window_shadow = egui::epaint::Shadow {
        offset: vec2(0.0, 6.0),
        blur: 20.0,
        spread: 0.0,
        color: Color32::from_black_alpha(130),
    };
    visuals.popup_shadow = egui::epaint::Shadow {
        offset: vec2(0.0, 3.0),
        blur: 10.0,
        spread: 0.0,
        color: Color32::from_black_alpha(110),
    };
    visuals.hyperlink_color = mix(accent, Color32::WHITE, 0.35);
    visuals.selection.bg_fill = mix(BG_INSET, accent, 0.6);
    visuals.selection.stroke = Stroke::new(1.0_f32, TEXT_STRONG);
    visuals.slider_trailing_fill = true;
    visuals.handle_shape = egui::style::HandleShape::Rect { aspect_ratio: 0.5 };
    visuals.striped = false;
    visuals.indent_has_left_vline = false;
    visuals.collapsing_header_frame = false;
    visuals.text_cursor.stroke = Stroke::new(1.5_f32, TEXT_STRONG);
    visuals.interact_cursor = None;
    visuals.button_frame = true;

    let rounding = Rounding::same(R_S);
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = BG_PANEL;
    widgets.noninteractive.weak_bg_fill = BG_PANEL;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, LINE);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    widgets.noninteractive.rounding = rounding;

    widgets.inactive.bg_fill = BG_FIELD;
    widgets.inactive.weak_bg_fill = BG_FIELD;
    widgets.inactive.bg_stroke = Stroke::new(1.0_f32, LINE_STRONG);
    widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    widgets.inactive.rounding = rounding;
    widgets.inactive.expansion = 0.0;

    widgets.hovered.bg_fill = BG_HOVER;
    widgets.hovered.weak_bg_fill = BG_HOVER;
    widgets.hovered.bg_stroke = Stroke::new(1.0_f32, LINE_STRONG);
    widgets.hovered.fg_stroke = Stroke::new(1.0_f32, TEXT_STRONG);
    widgets.hovered.rounding = rounding;
    widgets.hovered.expansion = 0.0;

    widgets.active.bg_fill = BG_ACTIVE;
    widgets.active.weak_bg_fill = BG_ACTIVE;
    widgets.active.bg_stroke = Stroke::new(1.0_f32, accent);
    widgets.active.fg_stroke = Stroke::new(1.0_f32, TEXT_STRONG);
    widgets.active.rounding = rounding;
    widgets.active.expansion = 0.0;

    widgets.open.bg_fill = BG_HOVER;
    widgets.open.weak_bg_fill = BG_HOVER;
    widgets.open.bg_stroke = Stroke::new(1.0_f32, LINE_STRONG);
    widgets.open.fg_stroke = Stroke::new(1.0_f32, TEXT_STRONG);
    widgets.open.rounding = rounding;

    ctx.set_style(style);
}
