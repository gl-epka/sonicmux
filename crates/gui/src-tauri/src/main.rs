//! SonicMux desktop executable entry point.

#![forbid(unsafe_code)]

fn main() {
    if let Err(error) = sonicmux_gui::run() {
        eprintln!("SonicMux could not start: {error}");
        std::process::exit(1);
    }
}
