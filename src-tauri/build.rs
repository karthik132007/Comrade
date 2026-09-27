fn main() {
    tauri_build::try_build(tauri_build::Attributes::new()).expect("tauri build script failed");
}
