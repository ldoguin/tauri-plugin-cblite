fn main() {
    // Pass no commands so we don't auto-generate allow/deny permissions that
    // would conflict with the hand-written ones in permissions/default.toml.
    // The builder still processes permissions/*.toml and emits the
    // cargo:PERMISSION_FILES_PATH metadata that tauri-build needs.
    tauri_plugin::Builder::new(&[]).android_path("android").ios_path("ios").build();

    // iOS: the plugin's Swift package depends on the CouchbaseLiteSwift binary
    // framework. swift-rs (patched to pass --triple) makes SwiftPM copy the
    // correct .xcframework slice into its products dir; the downstream cdylib
    // link of the app still needs an explicit framework search path + link so
    // the CouchbaseLiteSwift.* symbols referenced by our static lib resolve.
    // (The .app bundle gets the framework embedded via the Xcode project's own
    // SPM dependency, declared in src-tauri/gen/apple/project.yml.)
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios") {
        let out_dir = std::env::var("OUT_DIR").unwrap();
        let target = std::env::var("TARGET").unwrap();
        let simulator = target.ends_with("ios-sim")
            || (target.starts_with("x86_64") && target.ends_with("ios"));
        let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
            "aarch64" => "arm64".to_string(),
            other => other.to_string(),
        };
        let products_dir = format!(
            "{arch}-apple-ios{}",
            if simulator { "-simulator" } else { "" }
        );
        let config = if std::env::var("DEBUG").as_deref() == Ok("true") {
            "debug"
        } else {
            "release"
        };
        println!(
            "cargo:rustc-link-search=framework={out_dir}/swift-rs/tauri-plugin-cblite/{products_dir}/{config}"
        );
        println!("cargo:rustc-link-lib=framework=CouchbaseLiteSwift");
    }
}
