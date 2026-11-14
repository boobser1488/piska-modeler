//! КОПИЯ `crates/engine-editor/src/metrics.rs` движка Piska — не править руками.
//!
//! Обновить: `python scripts/sync_from_engine.py --engine <движок>`.
#![allow(dead_code, unused_imports)]

// Замер интерфейса (`kit::metrics`): после кадра без окна — все виджеты кадра (выделенный
// прямоугольник, чувствительность), все нарисованные рамки (с обводкой) и сводка: высоты
// контролов, строки с разной высотой или центром, рамки, вылезающие за свой виджет.
// Им пользуются проверки раскладки редактора, хаба и Modeler.

use std::collections::BTreeMap;

use egui::epaint::{ClippedShape, Shape};
use egui::{Id, LayerId, Rect};

/// Виджет кадра.
#[derive(Clone, Debug)]
pub struct WidgetMetric {
    pub id: Id,
    pub layer: LayerId,
    /// Выделенный прямоугольник.
    pub rect: Rect,
    /// Нажимается или тянется (контрол), а не просто подпись.
    pub interactive: bool,
    pub enabled: bool,
}

/// Нарисованная рамка (прямоугольник с обводкой): видимые границы с учётом обводки.
#[derive(Clone, Copy, Debug)]
pub struct PaintedFrame {
    pub rect: Rect,
    pub visual: Rect,
    pub stroke: f32,
}

/// Виджеты последнего кадра `ctx`.
pub fn widgets(ctx: &egui::Context) -> Vec<WidgetMetric> {
    ctx.viewport(|v| {
        v.prev_pass
            .widgets
            .layers()
            .flat_map(|(layer, rects)| {
                rects.iter().map(move |w| WidgetMetric {
                    id: w.id,
                    layer: *layer,
                    rect: w.rect,
                    interactive: w.sense.click || w.sense.drag,
                    enabled: w.enabled,
                })
            })
            .collect()
    })
}

/// Рамки с обводкой из нарисованных фигур кадра.
pub fn frames(shapes: &[ClippedShape]) -> Vec<PaintedFrame> {
    fn walk(shape: &Shape, out: &mut Vec<PaintedFrame>) {
        match shape {
            Shape::Vec(list) => list.iter().for_each(|s| walk(s, out)),
            Shape::Rect(r) if r.stroke.width > 0.0 && r.stroke.color.a() > 0 => {
                out.push(PaintedFrame { rect: r.rect, visual: r.visual_bounding_rect(), stroke: r.stroke.width })
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

/// Сводка замера одного экрана.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Высота контрола (×10, целым) → сколько.
    pub heights: BTreeMap<i32, usize>,
    /// Горизонтальный промежуток между соседями строки (×10) → сколько.
    pub gaps: BTreeMap<i32, usize>,
    /// Найденные нарушения: что и где.
    pub offenders: Vec<String>,
}

impl Report {
    pub fn merge(&mut self, other: Report) {
        for (k, v) in other.heights {
            *self.heights.entry(k).or_default() += v;
        }
        for (k, v) in other.gaps {
            *self.gaps.entry(k).or_default() += v;
        }
        self.offenders.extend(other.offenders);
    }

    /// Таблица «значение → сколько» для отчёта.
    pub fn table(map: &BTreeMap<i32, usize>) -> String {
        map.iter().map(|(k, v)| format!("| {:.1} | {v} |", *k as f32 / 10.0)).collect::<Vec<_>>().join("\n")
    }
}

/// Контрол строки: интерактивный, высотой как поле или кнопка (не панель и не вид сцены).
fn is_control(w: &WidgetMetric) -> bool {
    w.interactive && (12.0..=40.0).contains(&w.rect.height()) && w.rect.width() >= 10.0 && w.rect.width() <= 600.0
}

/// Разобрать кадр: `widgets` — [`widgets`], `frames` — [`frames`] того же кадра.
pub fn analyze(screen: &str, widgets: &[WidgetMetric], frames: &[PaintedFrame]) -> Report {
    let mut report = Report::default();
    let controls: Vec<&WidgetMetric> = widgets.iter().filter(|w| is_control(w)).collect();
    // Контрол с рамкой (кнопка, поле, список…) — по нарисованной рамке почти его размера;
    // текст-ссылки и строки списков без рамки в сравнение строк не входят.
    let framed = |w: &WidgetMetric| {
        frames.iter().any(|f| (w.rect.center() - f.rect.center()).length() < 1.5 && (w.rect.width() - f.rect.width()).abs() <= 2.5 && (w.rect.height() - f.rect.height()).abs() <= 2.5)
    };
    let controls: Vec<&WidgetMetric> = controls.into_iter().filter(|w| framed(w)).collect();
    for w in &controls {
        *report.heights.entry((w.rect.height() * 10.0).round() as i32).or_default() += 1;
        if (w.rect.height() - super::kit::CONTROL_H).abs() > 0.5 {
            report.offenders.push(format!("{screen}: control {:?} is {:.1} tall, not CONTROL_H: {:?}", w.id, w.rect.height(), w.rect));
        }
    }
    // Строки: контролы одного слоя, перекрывающиеся по вертикали больше чем наполовину и
    // стоящие рядом по горизонтали.
    for (i, a) in controls.iter().enumerate() {
        let next = controls
            .iter()
            .enumerate()
            .filter(|(j, b)| *j != i && b.layer == a.layer && b.rect.left() >= a.rect.right() - 0.5 && b.rect.left() - a.rect.right() <= 24.0)
            .filter(|(_, b)| {
                let overlap = a.rect.bottom().min(b.rect.bottom()) - a.rect.top().max(b.rect.top());
                overlap > 0.5 * a.rect.height().min(b.rect.height())
            })
            .min_by(|x, y| x.1.rect.left().total_cmp(&y.1.rect.left()));
        if let Some((_, b)) = next {
            *report.gaps.entry(((b.rect.left() - a.rect.right()) * 10.0).round() as i32).or_default() += 1;
            if (a.rect.height() - b.rect.height()).abs() > 0.5 || (a.rect.center().y - b.rect.center().y).abs() > 0.5 {
                report.offenders.push(format!(
                    "{screen}: row members differ: {:?} h{:.1} cy{:.1} vs {:?} h{:.1} cy{:.1}",
                    a.id,
                    a.rect.height(),
                    a.rect.center().y,
                    b.id,
                    b.rect.height(),
                    b.rect.center().y
                ));
            }
        }
    }
    // Рамка вылезает за свой контрол (обводка по краю, расширение при наведении).
    for f in frames {
        let owner = controls.iter().find(|w| {
            (w.rect.center() - f.rect.center()).length() < 1.0 && (w.rect.width() - f.rect.width()).abs() <= 2.0 && (w.rect.height() - f.rect.height()).abs() <= 2.0
        });
        if let Some(w) = owner {
            if !w.rect.expand(0.25).contains_rect(f.visual) {
                report.offenders.push(format!(
                    "{screen}: frame {:?} (stroke {:.1}) paints outside its control {:?} {:?}",
                    f.visual, f.stroke, w.id, w.rect
                ));
            }
        }
    }
    report
}

// ---------------------------------------------------------------------------
// Полный замер по правилам плотности: контейнеры, текст, промежутки.
// ---------------------------------------------------------------------------

/// Правила плотности для [`analyze_shapes`].
#[derive(Clone, Copy, Debug)]
pub struct Rules {
    /// Единственная допустимая внешняя высота контрола с рамкой.
    pub control_h: f32,
    /// Наименьший отступ текста от левого и правого края своего контрола.
    pub min_text_inset: f32,
    /// Проверять наложение чужого текста на контрол (выключают на экранах с открытым меню).
    pub text_overlap: bool,
}

impl Rules {
    /// Редактор и Modeler.
    pub fn compact() -> Self {
        Rules { control_h: super::kit::CONTROL_H, min_text_inset: super::kit::SP_2, text_overlap: true }
    }
    /// Хаб и установщик.
    pub fn comfortable() -> Self {
        Rules { control_h: super::kit::CONTROL_H_COMFORTABLE, min_text_inset: super::kit::SP_2, text_overlap: true }
    }
}

/// Допустимые промежутки между соседями строки: шкала отступов (и встык).
pub const GAP_SCALE: [f32; 8] = [0.0, super::kit::SP_1, super::kit::SP_2, super::kit::SP_3, super::kit::SP_4, super::kit::SP_6, super::kit::SP_8, super::kit::SP_12];

/// Фигура кадра в порядке рисования.
#[derive(Clone, Copy, Debug)]
enum Painted {
    /// Прямоугольник: (прямоугольник, видимые границы, обводка).
    Rect(Rect, Rect, f32),
    /// Текст: видимые границы глифов.
    Text(Rect),
}

fn painted(shapes: &[ClippedShape]) -> Vec<(Painted, Rect)> {
    fn walk(shape: &Shape, clip: Rect, out: &mut Vec<(Painted, Rect)>) {
        match shape {
            Shape::Vec(list) => list.iter().for_each(|s| walk(s, clip, out)),
            Shape::Rect(r) => {
                let stroked = r.stroke.width > 0.0 && r.stroke.color.a() > 0;
                let filled = r.fill.a() > 0;
                if stroked || filled {
                    out.push((Painted::Rect(r.rect, if stroked { r.visual_bounding_rect() } else { r.rect }, if stroked { r.stroke.width } else { 0.0 }), clip));
                }
            }
            Shape::Text(t) if !t.galley.is_empty() => {
                let bounds = t.galley.mesh_bounds.translate(t.pos.to_vec2());
                if bounds.is_positive() {
                    out.push((Painted::Text(bounds), clip));
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, clipped.clip_rect, &mut out);
    }
    out
}

/// Поверхность под контролом: последний прямоугольник, нарисованный до `before`, который
/// содержит центр `rect`, заметно больше его по площади и не его собственная рамка. Кнопка
/// шире своей карточки тоже ловится: размер сравнивается по площади, а не по сторонам.
fn surface(shapes: &[(Painted, Rect)], before: usize, rect: Rect) -> Option<usize> {
    shapes[..before].iter().enumerate().rev().find_map(|(i, (p, _))| match p {
        Painted::Rect(r, ..)
            if r.contains(rect.center())
                && r.area() > rect.area() * 1.5
                && !((r.center() - rect.center()).length() < 1.5 && (r.size() - rect.size()).length() < 3.6) =>
        {
            Some(i)
        }
        _ => None,
    })
}

/// Поверхность под текстом: последний прямоугольник до `before`, целиком его содержащий
/// (рамка подсказки лишь чуть больше своего текста).
fn text_surface(shapes: &[(Painted, Rect)], before: usize, text: Rect) -> Option<usize> {
    shapes[..before].iter().enumerate().rev().find_map(|(i, (p, _))| match p {
        Painted::Rect(r, ..) if r.expand(0.5).contains_rect(text) => Some(i),
        _ => None,
    })
}

/// Полный разбор кадра по правилам `rules`: высоты, строки (верх, низ, центр), промежутки
/// из шкалы, рамка внутри контрола, контрол внутри своего контейнера (карточка, панель,
/// диалог, полоса, строка меню — любой залитый или обведённый прямоугольник, нарисованный
/// раньше и содержащий центр контрола), текст контрола не ближе `min_text_inset` к краям,
/// чужой текст не заходит под контрол.
pub fn analyze_shapes(screen: &str, widgets: &[WidgetMetric], shapes: &[ClippedShape], rules: Rules) -> Report {
    let mut report = Report::default();
    let shapes = painted(shapes);
    // Контролы с рамкой: виджет и его рамка (индекс в порядке рисования).
    let mut controls: Vec<(&WidgetMetric, usize, Rect)> = Vec::new();
    for w in widgets.iter().filter(|w| is_control(w)) {
        let own = shapes.iter().enumerate().find(|(_, (p, _))| match p {
            Painted::Rect(rect, _, stroke) => {
                *stroke > 0.0 && (w.rect.center() - rect.center()).length() < 1.5 && (w.rect.width() - rect.width()).abs() <= 2.5 && (w.rect.height() - rect.height()).abs() <= 2.5
            }
            _ => false,
        });
        if let Some((index, (_, clip))) = own {
            // Наполовину прокрученный за край области контрол не меряется.
            if clip.expand(0.5).contains_rect(w.rect) && !controls.iter().any(|(c, ..)| c.rect == w.rect) {
                controls.push((w, index, *clip));
            }
        }
    }
    for (w, ..) in &controls {
        *report.heights.entry((w.rect.height() * 10.0).round() as i32).or_default() += 1;
        if (w.rect.height() - rules.control_h).abs() > 0.5 {
            report.offenders.push(format!("{screen}: control {:?} is {:.1} tall, not {:.0}: {:?}", w.id, w.rect.height(), rules.control_h, w.rect));
        }
    }
    // Строки: соседи справа, перекрытие по вертикали больше половины.
    for (i, (a, ..)) in controls.iter().enumerate() {
        let next = controls
            .iter()
            .enumerate()
            .filter(|(j, (b, ..))| *j != i && b.layer == a.layer && b.rect.left() >= a.rect.right() - 0.5 && b.rect.left() - a.rect.right() <= 32.0)
            .filter(|(_, (b, ..))| {
                let overlap = a.rect.bottom().min(b.rect.bottom()) - a.rect.top().max(b.rect.top());
                overlap > 0.5 * a.rect.height().min(b.rect.height())
            })
            .min_by(|x, y| x.1 .0.rect.left().total_cmp(&y.1 .0.rect.left()));
        if let Some((_, (b, ..))) = next {
            let gap = b.rect.left() - a.rect.right();
            *report.gaps.entry((gap * 10.0).round() as i32).or_default() += 1;
            if (a.rect.top() - b.rect.top()).abs() > 0.5 || (a.rect.bottom() - b.rect.bottom()).abs() > 0.5 {
                report.offenders.push(format!(
                    "{screen}: row members differ: {:?} top{:.1} bottom{:.1} vs {:?} top{:.1} bottom{:.1}",
                    a.id,
                    a.rect.top(),
                    a.rect.bottom(),
                    b.id,
                    b.rect.top(),
                    b.rect.bottom()
                ));
            }
            // Промежуток больше SP_8 между соседями — свободное место между группами
            // (левая и правая части полосы), а не отступ внутри строки.
            if gap <= super::kit::SP_8 + 0.5 && !GAP_SCALE.iter().any(|g| (gap - g).abs() < 0.6) {
                report.offenders.push(format!("{screen}: gap {gap:.1} between {:?} and {:?} is not on the spacing scale", a.id, b.id));
            }
        }
    }
    for (w, own, _) in &controls {
        // Рамка внутри контрола.
        if let (Painted::Rect(_, visual, stroke), _) = shapes[*own] {
            if !w.rect.expand(0.25).contains_rect(visual) {
                report.offenders.push(format!("{screen}: frame {visual:?} (stroke {stroke:.1}) paints outside its control {:?} {:?}", w.id, w.rect));
            }
        }
        // Контейнер: последний нарисованный до рамки прямоугольник, содержащий центр контрола
        // и заметно больше его (карточка поверх панели, диалог поверх страницы).
        if let Some(index) = surface(&shapes, *own, w.rect) {
            let Painted::Rect(rect, ..) = shapes[index].0 else { unreachable!() };
            if !rect.expand(0.25).contains_rect(w.rect) {
                report.offenders.push(format!("{screen}: control {:?} {:?} leaves its container {rect:?}", w.id, w.rect));
            }
        }
        // Свой текст: глифы, нарисованные после рамки, с центром внутри контрола.
        for (offset, (p, clip)) in shapes[*own + 1..].iter().enumerate() {
            if let Painted::Text(t) = p {
                let t = t.intersect(*clip);
                if !t.is_positive() || !w.rect.contains(t.center()) {
                    continue;
                }
                // Текст на поверхности поверх контрола (подсказка) — не его подпись.
                if text_surface(&shapes, *own + 1 + offset, t).is_some_and(|s| s > *own) {
                    continue;
                }
                let inner = w.rect.shrink2(egui::vec2(rules.min_text_inset - 0.5, 0.0));
                if !inner.contains_rect(t) {
                    report.offenders.push(format!("{screen}: text {t:?} is closer than {:.0} to the edge of {:?} {:?}", rules.min_text_inset, w.id, w.rect));
                }
            }
        }
    }
    // Чужой текст под контролом (подпись строки уходит под кнопку). Текст, нарисованный
    // раньше поверхности контрола (страница под диалогом), закрыт ею — не в счёт.
    if rules.text_overlap {
        for (ti, (p, clip)) in shapes.iter().enumerate() {
            let Painted::Text(t) = p else { continue };
            let t = t.intersect(*clip);
            if !t.is_positive() {
                continue;
            }
            for (w, own, _) in &controls {
                if w.rect.contains(t.center()) {
                    continue;
                }
                // Разные поверхности: текст под поверхностью контрола (страница под диалогом)
                // или контрол под поверхностью текста (кнопка страницы под диалогом, подсказка
                // поверх соседей) — не наложение в одной строке.
                if surface(&shapes, *own, w.rect).is_some_and(|s| ti < s) || text_surface(&shapes, ti, t).is_some_and(|s| s > *own) {
                    continue;
                }
                let overlap = t.intersect(w.rect.shrink(1.0));
                if overlap.is_positive() && overlap.area() > 2.0 {
                    report.offenders.push(format!("{screen}: text {t:?} runs under control {:?} {:?}", w.id, w.rect));
                }
            }
        }
    }
    report
}

/// Состояния контролов: каждому контролу с рамкой экрана — наведение, нажатие (отпускание
/// вдали, без щелчка) и фокус клавиатуры; после каждого — [`analyze_shapes`]. `frame` рисует
/// один кадр с событиями и возвращает его фигуры. Возвращает отчёт и число проверенных
/// состояний.
pub fn analyze_states(
    screen: &str,
    ctx: &egui::Context,
    rules: Rules,
    mut frame: impl FnMut(Vec<egui::Event>) -> Vec<ClippedShape>,
) -> (Report, usize) {
    let base = frame(Vec::new());
    let shapes_now = painted(&base);
    let targets: Vec<(Id, Rect)> = widgets(ctx)
        .into_iter()
        .filter(|w| is_control(w) && w.enabled)
        .filter(|w| {
            shapes_now.iter().any(|(p, clip)| match p {
                Painted::Rect(rect, _, stroke) => *stroke > 0.0 && clip.contains_rect(w.rect) && (w.rect.center() - rect.center()).length() < 1.5 && (w.rect.size() - rect.size()).length() < 3.6,
                _ => false,
            })
        })
        .map(|w| (w.id, w.rect))
        .collect();
    let mut report = Report::default();
    // Нарушения покоя уже в отчёте экрана: в состояниях — только новые.
    let mut seen: std::collections::BTreeSet<String> = analyze_shapes(screen, &widgets(ctx), &base, rules).offenders.into_iter().collect();
    let mut states = 0;
    let mut take = |report: &mut Report, state: &str, shapes: Vec<ClippedShape>| {
        let r = analyze_shapes(screen, &widgets(ctx), &shapes, rules);
        for o in r.offenders {
            // Одно нарушение — один раз, с первым состоянием, где оно видно.
            if seen.insert(o.clone()) {
                let tagged = o.replacen(&format!("{screen}:"), &format!("{screen} [{state}]:"), 1);
                report.offenders.push(tagged);
            }
        }
    };
    let away = egui::pos2(-50.0, -50.0);
    for (id, rect) in targets {
        let at = rect.center();
        frame(vec![egui::Event::PointerMoved(at)]);
        let shapes = frame(Vec::new());
        take(&mut report, &format!("hover {id:?}"), shapes);
        let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        let shapes = frame(vec![press(true)]);
        take(&mut report, &format!("press {id:?}"), shapes);
        // Увести указатель далеко и отпустить там: перетаскивание, не щелчок.
        frame(vec![egui::Event::PointerMoved(away)]);
        frame(vec![egui::Event::PointerButton { pos: away, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE }, egui::Event::PointerGone]);
        ctx.memory_mut(|m| m.request_focus(id));
        frame(Vec::new());
        let shapes = frame(Vec::new());
        take(&mut report, &format!("focus {id:?}"), shapes);
        ctx.memory_mut(|m| m.surrender_focus(id));
        frame(Vec::new());
        states += 3;
    }
    (report, states)
}
