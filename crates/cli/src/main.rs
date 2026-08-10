#![doc = "Command-line entry point for SonicMux."]
#![forbid(unsafe_code)]

use color_eyre::eyre::Result;

fn main() -> Result<()> {
    color_eyre::install()?;
    sonicmux_runtime::observability::init_tracing()?;

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        domain = sonicmux_core::CRATE_NAME,
        "starting SonicMux CLI skeleton"
    );
    println!("SonicMux CLI skeleton; commands arrive in M4.");

    Ok(())
}
