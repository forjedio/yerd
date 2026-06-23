fn main() {
    tauri_build::build();

    // macOS: link ServiceManagement so `smappservice.rs` can reach the
    // `SMAppService` ObjC class (used to register the daemon LaunchAgent — see
    // src/smappservice.rs). Gated on the *target* OS via CARGO_CFG_TARGET_OS
    // (not a host `#[cfg]`) so cross-builds link it only for macOS. The floor is
    // macOS 13, where the class is always present, so a normal link is correct.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-lib=framework=ServiceManagement");
    }
}
