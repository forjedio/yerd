//! Build-time tray icon rasterisation + Tauri codegen.

fn main() {
    if let Err(e) = run() {
        eprintln!("build script failed: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    render_tray_mac_icon()?;
    tauri_build::build();

    // macOS: link ServiceManagement so smappservice.rs can reach the SMAppService
    // ObjC class (registers the daemon LaunchAgent). Gated on the target OS via
    // CARGO_CFG_TARGET_OS (not a host #[cfg]) so cross-builds link it only for
    // macOS. The macOS 13 floor guarantees the class is present, so a normal link
    // is correct.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-lib=framework=ServiceManagement");
    }
    Ok(())
}

/// Rasterise `icons/tray-mac.svg` into `OUT_DIR/tray-mac.png`.
///
/// macOS status-bar template icons use a 22pt canvas; we render at @4x (88px) so
/// Retina menu bars get a clean downscale instead of upscaling a tiny bitmap.
fn render_tray_mac_icon() -> Result<(), Box<dyn std::error::Error>> {
    const SVG: &str = "icons/tray-mac.svg";
    const OUT: &str = "tray-mac.png";
    /// Logical menu-bar icon size (pt) per Apple's status-item guidance.
    const PT: f32 = 22.0;
    /// Raster scale factor (@4x) for sharp display on Retina screens.
    const SCALE: f32 = 4.0;

    println!("cargo:rerun-if-changed={SVG}");

    let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let svg_path = manifest_dir.join(SVG);
    let svg_data = std::fs::read(&svg_path)?;

    let opt = usvg::Options {
        resources_dir: svg_path.parent().map(std::path::Path::to_path_buf),
        ..Default::default()
    };

    let tree = usvg::Tree::from_data(&svg_data, &opt)?;

    let width = (PT * SCALE).round() as u32;
    let height = (PT * SCALE).round() as u32;
    let sx = width as f32 / tree.size().width();
    let sy = height as f32 / tree.size().height();

    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| format!("failed to allocate {width}x{height} tray icon pixmap"))?;
    pixmap.fill(resvg::tiny_skia::Color::TRANSPARENT);

    let transform = resvg::tiny_skia::Transform::from_scale(sx, sy);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR")?);
    let out_path = out_dir.join(OUT);
    pixmap.save_png(&out_path)?;
    Ok(())
}
