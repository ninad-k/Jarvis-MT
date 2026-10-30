fn main() {
    // Only the two launcher commands exist. Listing them here makes Tauri
    // require an explicit permission for each one in capabilities/.
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(
            tauri_build::AppManifest::new().commands(&["get_backend_url", "connect"]),
        ),
    )
    .expect("failed to run tauri-build");
}
