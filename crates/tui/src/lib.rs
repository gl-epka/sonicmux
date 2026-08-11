#![doc = "Terminal user interface for SonicMux."]
#![forbid(unsafe_code)]

mod app;
mod args;
mod input;
mod model;
mod terminal;
mod ui;

pub use app::App;
pub use args::TuiArgs;
