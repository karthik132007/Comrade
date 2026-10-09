fn main() {
    // The executable lives in bin/, and private native libraries in
    // lib/comrade-desktop/, in both the Debian and portable distributions.
    // $ORIGIN also supports cargo's adjacent development voice libraries.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN:$ORIGIN/../lib/comrade-desktop");
    }
    tauri_build::try_build(tauri_build::Attributes::new()).expect("tauri build script failed");
}
