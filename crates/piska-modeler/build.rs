//! Значок Piska Modeler в exe на Windows и подсистема «GUI» — без окна консоли и в
//! отладочной, и в выпускной сборке (`PISKA_CONSOLE=1` — с консолью). На других системах
//! ничего не делает.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    engine_build::windows_icon_file("assets/icon.ico");
    engine_build::windows_subsystem();
}
