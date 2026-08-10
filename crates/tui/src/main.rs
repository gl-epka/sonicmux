#![doc = "Terminal user interface entry point for SonicMux."]
#![forbid(unsafe_code)]

use color_eyre::eyre::Result;

fn main() -> Result<()> {
    color_eyre::install()?;
    sonicmux_runtime::observability::init_tracing()?;

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        domain = sonicmux_core::CRATE_NAME,
        "starting SonicMux TUI skeleton"
    );
    println!("SonicMux TUI skeleton; interface arrives in M6.");

    Ok(())
}
