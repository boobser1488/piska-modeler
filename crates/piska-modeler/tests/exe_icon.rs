//! Значок Piska Modeler зашит в exe (Windows) и совпадает с `assets/icon.ico`.

#[test]
fn the_exe_carries_the_modeler_icon() {
    if !cfg!(windows) {
        return;
    }
    let exe = std::fs::read(env!("CARGO_BIN_EXE_piska-modeler")).expect("the program is built");
    let images = engine_build::read_exe_icon(&exe).expect("a valid PE file").expect("the exe has an icon resource");
    let expected = engine_build::parse_ico(include_bytes!("../assets/icon.ico")).unwrap();
    assert_eq!(images.len(), expected.len(), "every size of icon.ico is embedded");
    let sizes: Vec<u32> = images.iter().map(|i| i.width).collect();
    assert!(sizes.contains(&16) && sizes.contains(&256), "{sizes:?}");
}

/// Окно без консоли: exe собран с подсистемой «GUI» (если не просили `PISKA_CONSOLE=1`),
/// а сам тест — консольная программа.
#[test]
fn the_exe_is_a_gui_program_without_a_console() {
    if !cfg!(all(windows, target_env = "msvc")) || std::env::var("PISKA_CONSOLE").is_ok_and(|v| !v.is_empty() && v != "0") {
        return;
    }
    let exe = std::fs::read(env!("CARGO_BIN_EXE_piska-modeler")).expect("the program is built");
    assert_eq!(engine_build::read_exe_subsystem(&exe).unwrap(), engine_build::SUBSYSTEM_GUI);
    let me = std::fs::read(std::env::current_exe().unwrap()).unwrap();
    assert_eq!(engine_build::read_exe_subsystem(&me).unwrap(), engine_build::SUBSYSTEM_CONSOLE, "tests keep their console");
}
