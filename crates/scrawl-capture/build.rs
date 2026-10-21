use std::process::Command;

/// `native::sck` links ScreenCaptureKit through Swift, so the crate's own test binaries need the
/// Swift runtime on their rpath on macOS, as the Scrawl app's do (its `build.rs`).
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos")
        || std::env::var_os("CARGO_FEATURE_NATIVE").is_none()
    {
        return;
    }
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    if let Ok(output) = Command::new("xcode-select").arg("-p").output()
        && output.status.success()
        && let Ok(developer_dir) = String::from_utf8(output.stdout)
    {
        for runtime in ["swift-5.5/macosx", "swift/macosx"] {
            let swift_runtime = format!(
                "{}/Toolchains/XcodeDefault.xctoolchain/usr/lib/{runtime}",
                developer_dir.trim()
            );
            println!("cargo:rustc-link-arg=-Wl,-rpath,{swift_runtime}");
        }
    }
}
