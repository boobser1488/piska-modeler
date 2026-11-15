//! Вид на программном растеризаторе в окне без окна (русский, 150 %): первый же кадр рисует вид
//! в размер окна. Раньше egui считал первый кадр по прежнему размеру экрана (10 000 точек), вид
//! просил текстуру 9484×9838, а egui принимает не больше 2048 — тест падал, и снимки пришлось
//! переключить на видеокарту. Один тест — одно устройство.

use engine_modeler::mesh_ops::Primitive;
use engine_modeler::ui::{Command, MeshCmd, ModelerApp};
use piska_modeler::Headless;

#[test]
fn the_software_view_fits_the_window_from_the_first_frame_at_150_percent() {
    engine_i18n::set(engine_i18n::Language::Russian);
    let Ok(mut headless) = Headless::new([1920, 1200], 1.5, false) else {
        eprintln!("no GPU: skipped");
        engine_i18n::set(engine_i18n::Language::English);
        return;
    };
    let mut app = ModelerApp::new();
    app.autosave_seconds = 0.0;
    headless.attach(&mut app);
    // Первый кадр — без паники, размер вида не больше окна.
    headless.frame(&mut app, vec![]);
    let [w, h] = app.viewport_frame_size();
    assert!(w <= 1920 && h <= 1200 && w > 0 && h > 0, "the first frame draws a view of {w}x{h} for a 1920x1200 window");
    let rect = app.viewport_rect().unwrap();
    assert!(rect.width() <= 1280.0 && rect.height() <= 800.0, "the view rectangle {rect:?} is inside the 1280x800 point window");
    app.run(Command::Mesh(MeshCmd::Add(Primitive::Cube { size: 12.0 })));
    for _ in 0..3 {
        headless.frame(&mut app, vec![]);
    }
    let image = headless.image();
    assert_eq!((image.width(), image.height()), (1920, 1200));
    engine_i18n::set(engine_i18n::Language::English);
}
