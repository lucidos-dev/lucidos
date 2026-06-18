fn main() {
    // Packaged builds run the engine + Postgres as an always-on launchd service
    // via `Lucidos --service` (headless, no window). Route there before any
    // Tauri/AppKit init so the service process never paints a window or claims a
    // dock icon. The normal double-click launch falls through to the GUI client.
    if std::env::args().skip(1).any(|a| a == "--service") {
        std::process::exit(lucidos_app::run_service());
    }
    lucidos_app::run()
}
