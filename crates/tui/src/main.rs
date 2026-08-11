#![doc = "Terminal user interface entry point for SonicMux."]
#![forbid(unsafe_code)]

use clap::Parser;
use color_eyre::eyre::Result;
use sonicmux_tui::{App, TuiArgs};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    color_eyre::install()?;
    App::new(TuiArgs::parse())?.run().await
}
