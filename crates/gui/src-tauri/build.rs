//! Generates the Tauri context and command capability manifests.

fn main() {
    let attributes =
        tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
            "bootstrap",
            "pick_inputs",
            "pick_output_directory",
            "remove_items",
            "set_item_enabled",
            "update_settings",
            "start_batch",
            "cancel_batch",
            "retry_items",
            "choose_ffmpeg",
            "retry_toolchain",
        ]));
    if let Err(error) = tauri_build::try_build(attributes) {
        eprintln!("failed to prepare SonicMux GUI resources: {error}");
        std::process::exit(1);
    }
}
