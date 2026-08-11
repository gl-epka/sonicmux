#![doc = "Native SonicMux desktop shell and constrained IPC adapter."]
#![forbid(unsafe_code)]

mod dto;
mod service;

use std::path::PathBuf;

use dto::{
    AcceptedDto, BootstrapDto, GuiEventDto, PickKindDto, SessionSnapshotDto, SettingsDto,
    ToolchainStatusDto,
};
use service::GuiService;
use tauri::menu::{
    AboutMetadataBuilder, Menu, MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder,
};
use tauri::{AppHandle, DragDropEvent, Manager, State, WebviewEvent, WindowEvent, ipc::Channel};
use tauri_plugin_dialog::DialogExt;

#[tauri::command]
async fn bootstrap(
    on_event: Channel<GuiEventDto>,
    service: State<'_, GuiService>,
) -> Result<BootstrapDto, String> {
    Ok(service.bootstrap(on_event).await)
}

#[tauri::command]
async fn pick_inputs(
    kind: PickKindDto,
    app: AppHandle,
    service: State<'_, GuiService>,
) -> Result<SessionSnapshotDto, String> {
    let picker = app.dialog().file().set_title("Add Matroska media");
    let picked = tauri::async_runtime::spawn_blocking(move || match kind {
        PickKindDto::Files => picker
            .add_filter("Matroska video", &["mkv"])
            .blocking_pick_files()
            .unwrap_or_default(),
        PickKindDto::Directory => picker
            .blocking_pick_folder()
            .into_iter()
            .collect::<Vec<_>>(),
    })
    .await
    .map_err(|error| format!("native file picker stopped: {error}"))?;
    let roots = picked
        .into_iter()
        .map(|value| value.into_path().map_err(|error| error.to_string()))
        .collect::<Result<Vec<PathBuf>, String>>()?;
    service.add_roots(roots).await
}

#[tauri::command]
async fn pick_output_directory(
    app: AppHandle,
    service: State<'_, GuiService>,
) -> Result<SessionSnapshotDto, String> {
    let picker = app.dialog().file().set_title("Choose output directory");
    let picked = tauri::async_runtime::spawn_blocking(move || picker.blocking_pick_folder())
        .await
        .map_err(|error| format!("native folder picker stopped: {error}"))?;
    let Some(path) = picked else {
        return Ok(service.current_snapshot().await);
    };
    let path = path.into_path().map_err(|error| error.to_string())?;
    service.set_output_directory(path).await
}

#[tauri::command]
async fn remove_items(
    ids: Vec<u64>,
    service: State<'_, GuiService>,
) -> Result<SessionSnapshotDto, String> {
    service.remove_items(&ids).await
}

#[tauri::command]
async fn set_item_enabled(
    id: u64,
    enabled: bool,
    service: State<'_, GuiService>,
) -> Result<SessionSnapshotDto, String> {
    service.set_item_enabled(id, enabled).await
}

#[tauri::command]
async fn update_settings(
    settings: SettingsDto,
    service: State<'_, GuiService>,
) -> Result<SessionSnapshotDto, String> {
    service.update_settings(settings).await
}

#[tauri::command]
async fn start_batch(service: State<'_, GuiService>) -> Result<AcceptedDto, String> {
    service.start_batch().await
}

#[tauri::command]
async fn cancel_batch(service: State<'_, GuiService>) -> Result<AcceptedDto, String> {
    service.cancel_batch().await
}

#[tauri::command]
async fn retry_items(
    ids: Vec<u64>,
    service: State<'_, GuiService>,
) -> Result<SessionSnapshotDto, String> {
    service.retry_items(&ids).await
}

#[tauri::command]
async fn choose_ffmpeg(
    app: AppHandle,
    service: State<'_, GuiService>,
) -> Result<ToolchainStatusDto, String> {
    let picker = app
        .dialog()
        .file()
        .set_title("Choose the FFmpeg executable");
    let picked = tauri::async_runtime::spawn_blocking(move || picker.blocking_pick_file())
        .await
        .map_err(|error| format!("native file picker stopped: {error}"))?;
    let Some(path) = picked else {
        return Err("FFmpeg selection was cancelled".to_owned());
    };
    service
        .choose_ffmpeg(path.into_path().map_err(|error| error.to_string())?)
        .await
}

#[tauri::command]
async fn retry_toolchain(service: State<'_, GuiService>) -> Result<ToolchainStatusDto, String> {
    service.retry_system_toolchain().await
}

/// Starts the desktop application.
///
/// # Errors
///
/// Returns a Tauri runtime error when the native shell cannot start.
pub fn run() -> tauri::Result<()> {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .menu(desktop_menu)
        .setup(|app| {
            let bundled = app
                .path()
                .resource_dir()
                .ok()
                .map(|directory| directory.join("binaries"));
            app.manage(GuiService::load(bundled.as_deref()));
            Ok(())
        })
        .on_webview_event(|webview, event| {
            if let WebviewEvent::DragDrop(DragDropEvent::Drop { paths, .. }) = event {
                let paths = paths.clone();
                let service = webview.app_handle().state::<GuiService>().inner().clone();
                tauri::async_runtime::spawn(async move {
                    let _ignored = service.add_roots(paths).await;
                });
            }
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let window = window.clone();
                let service = window.app_handle().state::<GuiService>().inner().clone();
                tauri::async_runtime::spawn(async move {
                    if service.is_active().await {
                        let _ignored = service.cancel_batch().await;
                        while service.is_active().await {
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        }
                        let _ignored = window.destroy();
                    } else {
                        let _ignored = window.destroy();
                    }
                });
            }
        })
        .on_menu_event(|app, event| {
            let action = event.id().as_ref().to_owned();
            if matches!(
                action.as_str(),
                "add-files" | "add-directory" | "start-batch" | "cancel-batch" | "remove-selected"
            ) {
                let service = app.state::<GuiService>().inner().clone();
                tauri::async_runtime::spawn(async move {
                    service.send_menu_action(&action).await;
                });
            }
        })
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            pick_inputs,
            pick_output_directory,
            remove_items,
            set_item_enabled,
            update_settings,
            start_batch,
            cancel_batch,
            retry_items,
            choose_ffmpeg,
            retry_toolchain,
        ])
        .run(tauri::generate_context!())
}

fn desktop_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let add_files = MenuItemBuilder::with_id("add-files", "Add MKV Files…")
        .accelerator("CmdOrCtrl+O")
        .build(app)?;
    let add_directory = MenuItemBuilder::with_id("add-directory", "Add Directory…")
        .accelerator("CmdOrCtrl+Shift+O")
        .build(app)?;
    let remove = MenuItemBuilder::with_id("remove-selected", "Remove Selected")
        .accelerator("Delete")
        .build(app)?;
    let start = MenuItemBuilder::with_id("start-batch", "Start Conversion")
        .accelerator("CmdOrCtrl+Enter")
        .build(app)?;
    let cancel = MenuItemBuilder::with_id("cancel-batch", "Cancel Batch").build(app)?;
    let file = SubmenuBuilder::new(app, "File")
        .items(&[&add_files, &add_directory])
        .separator()
        .item(&remove)
        .separator()
        .quit()
        .build()?;
    let batch = SubmenuBuilder::new(app, "Batch")
        .items(&[&start, &cancel])
        .build()?;
    let about = PredefinedMenuItem::about(
        app,
        Some("About SonicMux"),
        Some(
            AboutMetadataBuilder::new()
                .name(Some("SonicMux"))
                .version(Some(env!("CARGO_PKG_VERSION")))
                .authors(Some(vec!["Gleb Nechaev and SonicMux contributors".to_owned()]))
                .comments(Some(
                    "MKV audio compatibility. Bundled FFmpeg 8.1.2 is separately licensed under LGPL-2.1-or-later.",
                ))
                .copyright(Some("Copyright © 2026 SonicMux contributors"))
                .license(Some(
                    "SonicMux: MIT OR Apache-2.0; bundled FFmpeg: LGPL-2.1-or-later",
                ))
                .website(Some("https://github.com/gl-epka/sonicmux"))
                .website_label(Some("SonicMux on GitHub"))
                .credits(Some(
                    "SonicMux contributors\n\nBundled FFmpeg 8.1.2 is separately licensed under LGPL-2.1-or-later. Complete source and notices accompany each release.",
                ))
                .build(),
        ),
    )?;
    let help = SubmenuBuilder::new(app, "Help").item(&about).build()?;
    MenuBuilder::new(app)
        .items(&[&file, &batch, &help])
        .build()
}

#[cfg(test)]
mod tests {
    #[test]
    fn gui_depends_on_shared_application_layers() {
        assert_eq!(sonicmux_core::CRATE_NAME, "sonicmux-core");
        assert_eq!(sonicmux_runtime::CRATE_NAME, "sonicmux-runtime");
    }
}
