fn main() {
    // Pass no commands so we don't auto-generate allow/deny permissions that
    // would conflict with the hand-written ones in permissions/default.toml.
    // The builder still processes permissions/*.toml and emits the
    // cargo:PERMISSION_FILES_PATH metadata that tauri-build needs.
    tauri_plugin::Builder::new(&[]).android_path("android").build();
}
