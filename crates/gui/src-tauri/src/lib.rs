#![doc = "Tauri-facing adapter for the future SonicMux GUI."]
#![forbid(unsafe_code)]

/// Returns the application layers available to future Tauri commands.
#[must_use]
pub const fn application_layers() -> [&'static str; 2] {
    [sonicmux_core::CRATE_NAME, sonicmux_runtime::CRATE_NAME]
}

#[cfg(test)]
mod tests {
    use super::application_layers;

    #[test]
    fn gui_depends_on_shared_application_layers() {
        assert_eq!(application_layers(), ["sonicmux-core", "sonicmux-runtime"]);
    }
}
