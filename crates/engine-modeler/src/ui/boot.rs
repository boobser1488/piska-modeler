//! КОПИЯ `crates/engine-editor/src/boot.rs` движка Piska — не править руками.
//!
//! Обновить: `python scripts/sync_from_engine.py --engine <движок>`.
#![allow(dead_code, unused_imports)]

// Запуск программы и долгие операции: заставка, ход работы, строка прогресса.
//
// Одна реализация для редактора, Piska Hub, установщика и Piska Modeler (у Modeler — копия,
// `scripts/sync_from_engine.py`; здесь нет ничего, что знает про редактор).
//
// * [`Progress`] — «что делается сейчас», доля готовности и просьба отменить: работа пишет в
//   него из любого потока, интерфейс читает каждый кадр.
// * [`Task`] — работа в фоновом потоке: результат или ошибка (паника тоже ошибка), ход.
// * [`Boot`] — запуск программы: окно сразу рисует заставку ([`LoadingScreen`]: знак, имя,
//   настоящий шаг, полоса), пока [`Task`] готовит главный экран; готово — экран заменяет
//   заставку, сбой — экран ошибки с текстом и кнопками (программа не «висит»).
// * [`progress_row`] — строка прогресса долгой операции внутри программы: что происходит,
//   полоса или бегущий отрезок, «Отмена», где отменять безопасно.
//
// Окно показывают после первого кадра в цветах темы ([`Boot::themed`]): ни белого, ни
// чёрного прямоугольника системы.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use egui::{pos2, vec2, Align2, Color32, Id, Rect, Sense};

use super::controls;
use engine_i18n::{tr_s, trf};
use super::kit as theme;
use super::kit::{ TEXT, TEXT_MUTED, TEXT_STRONG};

// ---------------------------------------------------------------------------
// Ход работы
// ---------------------------------------------------------------------------

#[derive(Default)]
struct ProgressInner {
    step: Mutex<String>,
    fraction: Mutex<Option<f32>>,
    cancelled: AtomicBool,
}

/// Ход работы: текущий шаг, доля (если известна), просьба отменить. Копии делят одно
/// состояние; писать можно из любого потока.
#[derive(Clone, Default)]
pub struct Progress {
    inner: Arc<ProgressInner>,
}

impl Progress {
    pub fn new() -> Self {
        Self::default()
    }

    /// Что делается сейчас (одна строка для заставки или строки прогресса).
    pub fn set_step(&self, text: impl Into<String>) {
        if let Ok(mut step) = self.inner.step.lock() {
            *step = text.into();
        }
    }

    /// Доля готовности 0…1; `None` — неизвестна (бегущий отрезок).
    pub fn set_fraction(&self, fraction: Option<f32>) {
        if let Ok(mut f) = self.inner.fraction.lock() {
            *f = fraction.map(|v| v.clamp(0.0, 1.0));
        }
    }

    pub fn step(&self) -> String {
        self.inner.step.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn fraction(&self) -> Option<f32> {
        self.inner.fraction.lock().ok().and_then(|f| *f)
    }

    /// Попросить работу остановиться (она смотрит [`Progress::is_cancelled`]).
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Relaxed)
    }
}

/// Работа в фоновом потоке с ходом и результатом.
pub struct Task<T> {
    rx: mpsc::Receiver<Result<T, String>>,
    progress: Progress,
    started: Instant,
    finished: bool,
}

impl<T: Send + 'static> Task<T> {
    /// Запустить `work` в потоке `label`. `step` — первая строка хода. Паника в работе —
    /// ошибка с её текстом, а не вечное ожидание.
    pub fn spawn(label: &str, step: &str, work: impl FnOnce(&Progress) -> Result<T, String> + Send + 'static) -> Self {
        let progress = Progress::new();
        progress.set_step(step);
        let (tx, rx) = mpsc::channel();
        let worker = progress.clone();
        let spawned = std::thread::Builder::new().name(label.to_string()).spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&worker)));
            let outcome = outcome.unwrap_or_else(|payload| {
                let text = payload
                    .downcast_ref::<&str>()
                    .map(|s| (*s).to_string())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".to_string());
                Err(format!("the loader crashed: {text}"))
            });
            let _ = tx.send(outcome);
        });
        let (tx_err, rx) = match spawned {
            Ok(_) => (None, rx),
            Err(err) => {
                let (tx2, rx2) = mpsc::channel();
                (Some((tx2, err)), rx2)
            }
        };
        if let Some((tx2, err)) = tx_err {
            let _ = tx2.send(Err(format!("cannot start a background thread: {err}")));
        }
        Self { rx, progress, started: Instant::now(), finished: false }
    }
}

impl<T> Task<T> {
    pub fn progress(&self) -> &Progress {
        &self.progress
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Работа кончилась (результат уже забран).
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Результат, если он готов (один раз).
    pub fn poll(&mut self) -> Option<Result<T, String>> {
        if self.finished {
            return None;
        }
        match self.rx.try_recv() {
            Ok(result) => {
                self.finished = true;
                Some(result)
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.finished = true;
                Some(Err("the background work ended without a result".to_string()))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Полоса прогресса и заставка
// ---------------------------------------------------------------------------

/// Тонкая полоса прогресса: с долей — заполнена до неё, без — бегущий отрезок.
pub fn paint_progress(painter: &egui::Painter, bar: Rect, fraction: Option<f32>, time: f64, alpha: f32) {
    let fade = |c: Color32| c.gamma_multiply(alpha);
    painter.rect_filled(bar, 1.0, fade(theme::BG_FIELD));
    let segment = match fraction {
        Some(f) => Rect::from_min_max(bar.min, pos2(bar.left() + bar.width() * f.clamp(0.0, 1.0), bar.bottom())),
        None => {
            let t = (time * 0.9).fract() as f32;
            let eased = t * t * (3.0 - 2.0 * t);
            let width = bar.width() * 0.3;
            let x = bar.left() - width + (bar.width() + width) * eased;
            Rect::from_min_max(pos2(x.max(bar.left()), bar.top()), pos2((x + width).min(bar.right()), bar.bottom()))
        }
    };
    if segment.width() > 0.0 {
        painter.rect_filled(segment, 1.0, fade(theme::accent()));
    }
}

/// Рисует знак программы в квадрате с прозрачностью.
pub type MarkFn = fn(&egui::Painter, Rect, f32);

/// Заставка загрузки программы (редактор, Piska Hub, установщик, Piska Modeler): знак, имя,
/// версия, **настоящий** текущий шаг и полоса (бегущий отрезок или доля); держится, пока идёт
/// работа (не короче [`LoadingScreen::MIN_SECONDS`], если её вообще было видно),
/// растворяется и ввод не перехватывает.
#[derive(Clone, Debug)]
pub struct LoadingScreen {
    started: Option<f64>,
    fading_since: Option<f64>,
    done: bool,
    hold: f64,
}

impl Default for LoadingScreen {
    fn default() -> Self {
        Self { started: None, fading_since: None, done: false, hold: Self::MIN_SECONDS }
    }
}

impl LoadingScreen {
    pub const MIN_SECONDS: f64 = 0.6;
    pub const MAX_SECONDS: f64 = 30.0;
    pub const FADE_SECONDS: f64 = 0.3;

    /// Заставка, которая растворяется сразу, как кончилась работа (дальше своя заставка
    /// программы с тем же видом).
    pub fn without_hold() -> Self {
        Self { hold: 0.0, ..Self::default() }
    }

    /// Заставка, которой уже нет (программе она не нужна).
    pub fn dismissed() -> Self {
        Self { done: true, ..Self::default() }
    }

    /// Заставка ещё на экране.
    pub fn visible(&self) -> bool {
        !self.done
    }

    /// Нарисовать кадр заставки поверх всего с бегущим отрезком (см. [`LoadingScreen::show_with`]).
    pub fn show(&mut self, ctx: &egui::Context, mark: &dyn Fn(&egui::Painter, Rect, f32), name: &str, version: &str, step: Option<&str>) {
        self.show_with(ctx, mark, name, version, step, None);
    }

    /// Нарисовать кадр заставки поверх всего. `step` — что делается сейчас (`None` —
    /// загрузка кончилась), `fraction` — доля, если она известна.
    pub fn show_with(
        &mut self,
        ctx: &egui::Context,
        mark: &dyn Fn(&egui::Painter, Rect, f32),
        name: &str,
        version: &str,
        step: Option<&str>,
        fraction: Option<f32>,
    ) {
        if self.done {
            return;
        }
        let now = ctx.input(|i| i.time);
        let started = *self.started.get_or_insert(now);
        let elapsed = now - started;
        if self.fading_since.is_none() && ((step.is_none() && elapsed >= self.hold) || elapsed >= Self::MAX_SECONDS) {
            self.fading_since = Some(now);
        }
        let alpha = match self.fading_since {
            Some(since) => 1.0 - ((now - since) / Self::FADE_SECONDS).clamp(0.0, 1.0) as f32,
            None => 1.0,
        };
        if alpha <= 0.0 {
            self.done = true;
            return;
        }
        ctx.request_repaint();
        let screen = ctx.screen_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("loading_screen")));
        let fade = |c: Color32| c.gamma_multiply(alpha);
        painter.rect_filled(screen, 0.0, fade(theme::BG_APP));
        let center = screen.center() - vec2(0.0, 24.0);
        let square = Rect::from_center_size(center - vec2(0.0, 28.0), vec2(48.0, 48.0));
        mark(&painter, square, alpha);
        painter.text(pos2(center.x, square.bottom() + 18.0), Align2::CENTER_CENTER, name, theme::strong(theme::FONT_HEADING), fade(TEXT_STRONG));
        if let Some(step) = step {
            painter.text(pos2(center.x, square.bottom() + 38.0), Align2::CENTER_CENTER, step, theme::caption(), fade(TEXT_MUTED));
        }
        let bar = Rect::from_center_size(pos2(center.x, square.bottom() + 58.0), vec2(180.0, 2.0));
        paint_progress(&painter, bar, fraction, elapsed, alpha);
        painter.text(pos2(center.x, screen.bottom() - 20.0), Align2::CENTER_CENTER, version, theme::caption(), fade(theme::TEXT_DISABLED));
    }
}

// ---------------------------------------------------------------------------
// Строка прогресса долгой операции
// ---------------------------------------------------------------------------

/// Строка прогресса долгой операции внутри программы: подпись («что происходит»), полоса
/// (с долей или бегущий отрезок), процент и, если отменять безопасно, кнопка «Cancel».
/// Высота — высота контрола, ширина — вся доступная; кадры идут, пока строка на экране.
/// Возвращает `true`, когда нажали «Cancel».
pub fn progress_row(ui: &mut egui::Ui, label: &str, fraction: Option<f32>, cancellable: bool) -> bool {
    let h = theme::control_h(ui);
    let mut cancel = false;
    controls::row(ui, |ui| {
        let gap = theme::ITEM_GAP;
        let total = ui.available_width();
        let cancel_w = if cancellable { controls::button_width(ui, None, tr_s("Cancel")) + gap } else { 0.0 };
        // Столбец процента занят всегда: полосы строк с долей и без неё стоят ровно.
        let percent_w = PERCENT_W + gap;
        let bar_w = (total * 0.3).clamp(80.0, 220.0).min((total - cancel_w - percent_w - gap * 2.0).max(40.0));
        let label_w = (total - bar_w - cancel_w - percent_w - gap).max(40.0);
        let shown = ui.allocate_ui_with_layout(vec2(label_w, h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.add(egui::Label::new(egui::RichText::new(label).font(theme::body()).color(TEXT)).truncate())
        });
        if text_width(ui, label) > label_w {
            shown.response.on_hover_text(label);
        }
        let (bar_rect, _) = ui.allocate_exact_size(vec2(bar_w, h), Sense::hover());
        let bar = Rect::from_center_size(bar_rect.center(), vec2(bar_w, 4.0));
        paint_progress(ui.painter(), bar, fraction, ui.input(|i| i.time), 1.0);
        let (rect, _) = ui.allocate_exact_size(vec2(PERCENT_W, h), Sense::hover());
        if let Some(f) = fraction {
            ui.painter().text(
                pos2(rect.right(), rect.center().y),
                Align2::RIGHT_CENTER,
                format!("{}%", (f.clamp(0.0, 1.0) * 100.0).round() as u32),
                theme::caption(),
                TEXT_MUTED,
            );
        }
        if cancellable && controls::button(ui, tr_s("Cancel")).clicked() {
            cancel = true;
        }
    });
    ui.ctx().request_repaint_after(Duration::from_millis(33));
    cancel
}

/// Ширина столбца процента в строке прогресса («100%» в подписи размера caption).
const PERCENT_W: f32 = 34.0;

fn text_width(ui: &egui::Ui, text: &str) -> f32 {
    ui.painter().layout_no_wrap(text.to_string(), theme::body(), TEXT).size().x
}

// ---------------------------------------------------------------------------
// Запуск программы
// ---------------------------------------------------------------------------

/// Работа, которую нельзя унести в другой поток (значение не `Send`): её выполняет вызвавший
/// поток, когда заставка уже нарисована.
type LocalWork<T> = Box<dyn FnOnce(&Progress) -> Result<T, String>>;

enum Stage<T> {
    /// Работа в фоновом потоке.
    Loading(Task<T>),
    /// Работа ждёт, пока нарисуется заставка ([`Boot::start_local`]).
    Pending(LocalWork<T>, Progress),
    /// Работа идёт в потоке окна ([`Boot::begin_local`]): заставку живой держит помощник.
    Building(Progress),
    Ready(T),
    Failed(String),
}

/// Состояние запуска программы.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootState {
    Loading,
    Ready,
    Failed,
}

/// Запуск программы: заставка, пока [`Task`] готовит главный экран; готово — экран,
/// сбой — экран ошибки. Программа зовёт [`Boot::frame`] каждый кадр вместо своего интерфейса.
pub struct Boot<T> {
    stage: Stage<T>,
    screen: LoadingScreen,
    name: String,
    version: String,
    mark: MarkFn,
    themed_frames: u32,
    quit: bool,
}

impl<T> Boot<T> {
    /// Запуск с заставкой: работа уже идёт в `task`.
    pub fn start(name: &str, version: &str, mark: MarkFn, task: Task<T>) -> Self
    where
        T: Send + 'static,
    {
        Self { stage: Stage::Loading(task), screen: LoadingScreen::default(), name: name.to_string(), version: version.to_string(), mark, themed_frames: 0, quit: false }
    }

    /// Уже готовое значение без заставки (тесты, снимки): интерфейс рисуется сразу.
    pub fn ready(name: &str, version: &str, mark: MarkFn, value: T) -> Self {
        Self { stage: Stage::Ready(value), screen: LoadingScreen::dismissed(), name: name.to_string(), version: version.to_string(), mark, themed_frames: 0, quit: false }
    }

    /// Запуск с заставкой для работы, которую нельзя унести в другой поток (например редактор:
    /// его `App` не `Send`). Окно рисует заставку, и сразу после первого полного кадра
    /// вызывающий поток выполняет работу ([`Boot::should_build`], [`Boot::run_pending`]).
    pub fn start_local(name: &str, version: &str, mark: MarkFn, step: &str, work: impl FnOnce(&Progress) -> Result<T, String> + 'static) -> Self {
        let progress = Progress::new();
        progress.set_step(step);
        Self {
            stage: Stage::Pending(Box::new(work), progress),
            screen: LoadingScreen::default(),
            name: name.to_string(),
            version: version.to_string(),
            mark,
            themed_frames: 0,
            quit: false,
        }
    }

    /// Работа [`Boot::start_local`] ещё не выполнена, а заставка уже нарисована хотя бы раз:
    /// пора выполнять (сразу после показа кадра).
    pub fn should_build(&self) -> bool {
        matches!(self.stage, Stage::Pending(..)) && self.themed_frames >= 1
    }

    /// Выполнить отложенную работу здесь и сейчас (окно во время неё не отвечает — см.
    /// [`Boot::begin_local`], если заставку нужно держать живой).
    pub fn run_pending(&mut self) {
        if let Some((work, progress)) = self.begin_local() {
            let result = work(&progress);
            self.finish_local(result);
        }
    }

    /// Взять отложенную работу и её ход; пока она не закончена ([`Boot::finish_local`]),
    /// запуск считается идущим. Так программа запускает помощника, который рисует заставку
    /// из другого потока, пока этот поток занят.
    pub fn begin_local(&mut self) -> Option<(LocalWork<T>, Progress)> {
        match std::mem::replace(&mut self.stage, Stage::Failed(String::new())) {
            Stage::Pending(work, progress) => {
                self.stage = Stage::Building(progress.clone());
                Some((work, progress))
            }
            other => {
                self.stage = other;
                None
            }
        }
    }

    /// Результат работы из [`Boot::begin_local`].
    pub fn finish_local(&mut self, result: Result<T, String>) {
        self.stage = match result {
            Ok(value) => Stage::Ready(value),
            Err(message) => {
                log::error!("start-up failed: {message}");
                Stage::Failed(message)
            }
        };
    }

    /// Всё, что нужно помощнику, чтобы рисовать заставку из другого потока: имя, версия,
    /// знак и ход работы.
    pub fn splash_spec(&self) -> Option<SplashSpec> {
        let progress = self.progress()?.clone();
        Some(SplashSpec { name: self.name.clone(), version: self.version.clone(), mark: self.mark, progress })
    }

    /// Остановить запуск с ошибкой (например, не удалось подготовить видеокарту).
    pub fn fail(&mut self, message: impl Into<String>) {
        self.stage = Stage::Failed(message.into());
    }

    /// Заставка растворяется сразу, как работа кончилась (у программы дальше своя заставка
    /// с тем же видом, например редактор: первый кадр сцены).
    pub fn hand_over_when_ready(mut self) -> Self {
        self.screen = LoadingScreen::without_hold();
        self
    }

    /// Забрать результат, если он готов. `true` — состояние поменялось на этом вызове.
    pub fn poll(&mut self) -> bool {
        let Stage::Loading(task) = &mut self.stage else {
            return false;
        };
        match task.poll() {
            Some(Ok(value)) => self.stage = Stage::Ready(value),
            Some(Err(message)) => {
                log::error!("start-up failed: {message}");
                self.stage = Stage::Failed(message);
            }
            None => return false,
        }
        true
    }

    pub fn state(&self) -> BootState {
        match self.stage {
            Stage::Loading(_) | Stage::Pending(..) | Stage::Building(_) => BootState::Loading,
            Stage::Ready(_) => BootState::Ready,
            Stage::Failed(_) => BootState::Failed,
        }
    }

    /// Готовое значение.
    pub fn value(&self) -> Option<&T> {
        match &self.stage {
            Stage::Ready(value) => Some(value),
            _ => None,
        }
    }

    pub fn value_mut(&mut self) -> Option<&mut T> {
        match &mut self.stage {
            Stage::Ready(value) => Some(value),
            _ => None,
        }
    }

    /// Забрать готовое значение (после этого запуск — ошибка «значение забрано»).
    pub fn take_value(&mut self) -> Option<T> {
        match std::mem::replace(&mut self.stage, Stage::Failed("the start-up result was taken".to_string())) {
            Stage::Ready(value) => Some(value),
            other => {
                self.stage = other;
                None
            }
        }
    }

    /// Текст ошибки запуска.
    pub fn error(&self) -> Option<&str> {
        match &self.stage {
            Stage::Failed(message) => Some(message),
            _ => None,
        }
    }

    /// Текущий шаг загрузки, пока она идёт.
    pub fn step(&self) -> Option<String> {
        self.progress().map(Progress::step)
    }

    pub fn progress(&self) -> Option<&Progress> {
        match &self.stage {
            Stage::Loading(task) => Some(task.progress()),
            Stage::Pending(_, progress) | Stage::Building(progress) => Some(progress),
            _ => None,
        }
    }

    /// Заставка ещё на экране.
    pub fn splash_visible(&self) -> bool {
        self.screen.visible()
    }

    /// Нарисован ли хоть один полный кадр в цветах темы и со шрифтами: окно можно показать.
    pub fn themed(&self) -> bool {
        self.themed_frames > 0
    }

    /// Нажали «Close» на экране ошибки: программе пора выйти.
    pub fn quit_requested(&self) -> bool {
        self.quit
    }

    /// Один кадр интерфейса запуска. `ready` рисует главный экран программы (когда он
    /// готов) и возвращает шаг, если программе ещё нужно что-то догрузить (заставка тогда
    /// остаётся и пишет его); `None` — всё готово.
    pub fn frame(&mut self, ctx: &egui::Context, ready: impl FnOnce(&egui::Context, &mut T) -> Option<String>) {
        self.poll();
        if !theme_ready(ctx) {
            return;
        }
        self.themed_frames = self.themed_frames.saturating_add(1);
        let (name, version, mark) = (self.name.clone(), self.version.clone(), self.mark);
        match &mut self.stage {
            Stage::Ready(value) => {
                let step = ready(ctx, value);
                self.screen.show_with(ctx, &mark, &name, &version, step.as_deref(), None);
            }
            Stage::Loading(_) | Stage::Pending(..) | Stage::Building(_) => self.paint_splash(ctx, None),
            Stage::Failed(message) => {
                let message = message.clone();
                error_screen(ctx, &mark, &name, &version, &message, &mut self.quit);
            }
        }
    }

    /// Кадр заставки, даже когда значение уже готово (программа ещё ждёт чего-то своего:
    /// например видеокарту). `step` — что писать вместо шага работы.
    pub fn splash_frame(&mut self, ctx: &egui::Context, step: Option<&str>) {
        self.poll();
        if !theme_ready(ctx) {
            return;
        }
        self.themed_frames = self.themed_frames.saturating_add(1);
        match &self.stage {
            Stage::Failed(message) => {
                let message = message.clone();
                let (name, version, mark) = (self.name.clone(), self.version.clone(), self.mark);
                error_screen(ctx, &mark, &name, &version, &message, &mut self.quit);
            }
            _ => self.paint_splash(ctx, step),
        }
    }

    fn paint_splash(&mut self, ctx: &egui::Context, step: Option<&str>) {
        egui::CentralPanel::default().frame(egui::Frame::none().fill(theme::BG_APP)).show(ctx, |_| {});
        let (own_step, fraction) = match self.progress() {
            Some(p) => (p.step(), p.fraction()),
            None => (String::new(), None),
        };
        let shown = step.unwrap_or(&own_step).to_string();
        let (name, version, mark) = (self.name.clone(), self.version.clone(), self.mark);
        self.screen.show_with(ctx, &mark, &name, &version, Some(&shown), fraction);
        ctx.request_repaint_after(Duration::from_millis(33));
    }
}

/// Всё, что нужно, чтобы рисовать заставку отдельно от [`Boot`] (из потока-помощника).
#[derive(Clone)]
pub struct SplashSpec {
    pub name: String,
    pub version: String,
    pub mark: MarkFn,
    pub progress: Progress,
}

impl SplashSpec {
    /// Кадр заставки для `ctx`. `false` — тема ещё ставится (шрифты со следующего кадра):
    /// кадр пустой, показывать окно рано.
    pub fn frame(&self, ctx: &egui::Context, screen: &mut LoadingScreen) -> bool {
        if !theme_ready(ctx) {
            return false;
        }
        egui::CentralPanel::default().frame(egui::Frame::none().fill(theme::BG_APP)).show(ctx, |_| {});
        let step = self.progress.step();
        screen.show_with(ctx, &self.mark, &self.name, &self.version, Some(&step), self.progress.fraction());
        true
    }
}

/// Тема и шрифты уже действуют в `ctx`. Если нет — ставит их и рисует пустой фон темы: шрифты
/// подключаются только со следующего кадра, этот кадр показывать рано.
pub fn theme_ready(ctx: &egui::Context) -> bool {
    if theme::fonts_installed(ctx) {
        return true;
    }
    theme::apply(ctx, true);
    ctx.request_repaint();
    egui::CentralPanel::default().frame(egui::Frame::none().fill(theme::BG_APP)).show(ctx, |_| {});
    false
}

/// Экран ошибки запуска: знак, «не удалось запустить», текст ошибки и кнопки.
fn error_screen(ctx: &egui::Context, mark: &MarkFn, name: &str, version: &str, message: &str, quit: &mut bool) {
    egui::CentralPanel::default().frame(egui::Frame::none().fill(theme::BG_APP)).show(ctx, |ui| {
        let screen = ui.max_rect();
        let width = (screen.width() - theme::SP_8 * 2.0).clamp(240.0, 560.0);
        let top = (screen.height() * 0.5 - 150.0).max(theme::SP_8);
        let column = Rect::from_min_size(pos2(screen.center().x - width * 0.5, screen.top() + top), vec2(width, screen.height() - top));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(column).layout(egui::Layout::top_down(egui::Align::Center)));
        let (mark_rect, _) = child.allocate_exact_size(vec2(48.0, 48.0), Sense::hover());
        mark(child.painter(), mark_rect, 1.0);
        child.add_space(theme::SP_6);
        child.label(egui::RichText::new(trf!("{name} could not start", name = name)).font(theme::strong(theme::FONT_HEADING)).color(TEXT_STRONG));
        child.add_space(theme::SP_2);
        child.label(egui::RichText::new(tr_s("The program could not finish starting. The details below say what to fix; then start it again.")).font(theme::caption()).color(TEXT_MUTED));
        child.add_space(theme::SP_6);
        controls::card(&mut child, |ui| {
            ui.set_width(ui.available_width());
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.add(egui::Label::new(egui::RichText::new(message).font(theme::mono()).color(theme::ERROR)).selectable(true).wrap());
            });
        });
        child.add_space(theme::SP_6);
        // Кнопки по центру, одной строкой: высота — высота контрола (строка не растягивается на
        // остаток колонки).
        let (close, copy) = (tr_s("Close"), tr_s("Copy details"));
        let buttons_w = controls::button_width(&child, None, close) + theme::ITEM_GAP + controls::button_width(&child, None, copy);
        let row_h = theme::control_h(&child);
        child.allocate_ui_with_layout(vec2(width, row_h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.add_space(((width - buttons_w) * 0.5).max(0.0));
            ui.spacing_mut().item_spacing.x = theme::ITEM_GAP;
            if controls::primary_button(ui, close).clicked() {
                *quit = true;
            }
            if controls::button(ui, copy).clicked() {
                ui.ctx().copy_text(format!("{name} {version}\n{message}"));
            }
        });
    });
    ctx.request_repaint_after(Duration::from_millis(250));
}

/// Все тексты кадра в порядке рисования: так тесты проверяют, что на экране (заставка, шаг,
/// ошибка), не читая пиксели.
pub fn painted_texts(full: &egui::FullOutput) -> Vec<String> {
    fn walk(shape: &egui::epaint::Shape, out: &mut Vec<String>) {
        match shape {
            egui::epaint::Shape::Text(text) => out.push(text.galley.text().to_string()),
            egui::epaint::Shape::Vec(list) => list.iter().for_each(|s| walk(s, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &full.shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

/// Закрашенные прямоугольники кадра (прямоугольник, цвет) в порядке рисования: первый, что
/// накрывает весь экран, — фон окна.
pub fn painted_fills(full: &egui::FullOutput) -> Vec<(Rect, Color32)> {
    fn walk(shape: &egui::epaint::Shape, out: &mut Vec<(Rect, Color32)>) {
        match shape {
            egui::epaint::Shape::Rect(rect) => out.push((rect.rect, rect.fill)),
            egui::epaint::Shape::Vec(list) => list.iter().for_each(|s| walk(s, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &full.shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}
