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
