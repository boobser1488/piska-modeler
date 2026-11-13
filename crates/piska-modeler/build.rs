//! Значок Piska Modeler в exe на Windows (на других системах ничего не делает).

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    engine_build::windows_icon_file("assets/icon.ico");
}
