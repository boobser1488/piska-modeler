//! Piska Modeler — отдельное окно редактора моделей.
//!
//! ```text
//! cargo run -p piska-modeler -- [файлы.pmodel…]
//! cargo run -p piska-modeler -- assets/models/rover.pmodel --screenshot shot.png --size 1440x860
//! cargo run -p piska-modeler -- assets/models/rover.pmodel --bench --size 1920x1080
//! ```
//!
//! Ключи для снимков без окна: `--screenshot файл.png`, `--size ШxВ`, `--frames N`,
//! `--sample crate|lantern|pine|rover` (образец вместо файла), `--paint` (режим рисования),
//! `--tool <имя>`, `--view front|back|left|right|top|bottom`, `--ortho`, `--select <узел>`,
//! `--face north|east|south|west|up|down`, `--pointer X,Y` (курсор над окном), `--scale 1.25`,
//! `--cpu` (программный растеризатор вместо видеокарты). `--bench` — замер кадра.

fn main() {
    let options = match piska_modeler::parse_args(std::env::args().skip(1)) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let result = if options.bench {
        piska_modeler::bench(&options)
    } else if let Some(path) = options.screenshot.clone() {
        piska_modeler::screenshot(&options, &path)
    } else {
        piska_modeler::run_window(&options).map(|app| {
            if app.has_unsaved_changes() {
                eprintln!(
                    "piska-modeler: closed with unsaved changes; they stay in the recovery file and are offered on the next start"
                );
            }
        })
    };
    if let Err(e) = result {
        eprintln!("piska-modeler: {e}");
        std::process::exit(1);
    }
}
