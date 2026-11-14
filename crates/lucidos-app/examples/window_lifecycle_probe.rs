//! Entry point for the macOS window-lifecycle probe. The probe itself lives in
//! `src/window_probe.rs`, inside the crate, so it can call the real
//! traffic-light placement rather than a copy of it.
//!
//! ```text
//! cargo run -p lucidos-app --features window-probe \
//!     --example window_lifecycle_probe -- drive --bar 54
//! ```
//!
//! `watch` instead of `drive` leaves the window to you, for the transitions no
//! public API can script.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(lucidos_app::window_probe::run(&args));
}
