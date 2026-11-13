//! Записать образцы моделей (`.pmodel`) и, по желанию, их экспорт в `.glb`.
//!
//! ```text
//! cargo run -p engine-modeler --example gen_samples [каталог .pmodel] [--glb каталог]
//! ```
//!
//! По умолчанию — `samples/` репозитория Modeler. Движок Piska держит копии в
//! `assets/models/` и экспорт `rover.glb` для своего теста загрузчика:
//! `cargo run --manifest-path modeler/Cargo.toml -p engine-modeler --example gen_samples --
//! assets/models --glb crates/engine-assets/tests/fixtures`.
//!
//! Модели собираются тем же API, что и кнопки Piska Modeler (`engine_modeler::samples`).

use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../samples");
    let mut glb: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--glb" => glb = args.next().map(PathBuf::from),
            other => dir = PathBuf::from(other),
        }
    }
    for (name, doc) in engine_modeler::samples::all() {
        let path = dir.join(format!("{name}.{}", engine_modeler::format::EXTENSION));
        if let Err(e) = engine_modeler::format::save(&doc, &path) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        println!(
            "{}: {} elements, {} triangles",
            path.display(),
            doc.element_count(),
            engine_modeler::bake(&doc).triangle_count()
        );
        if let Some(out) = &glb {
            let _ = std::fs::create_dir_all(out);
            let path = out.join(format!("{name}.glb"));
            if let Err(e) = engine_modeler::export::write_glb(&doc, &path) {
                eprintln!("{e}");
                std::process::exit(1);
            }
            println!("{}", path.display());
        }
    }
}
