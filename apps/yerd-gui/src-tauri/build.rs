fn main() {
    tauri_build::build();

    // macOS: link ServiceManagement so smappservice.rs can reach the SMAppService
    // ObjC class (registers the daemon LaunchAgent). Gated on the target OS via
    // CARGO_CFG_TARGET_OS (not a host #[cfg]) so cross-builds link it only for
    // macOS. The macOS 13 floor guarantees the class is present, so a normal link
    // is correct.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-lib=framework=ServiceManagement");
    }
}
