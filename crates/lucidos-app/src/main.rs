use lucidos_app::LaunchRole;

fn main() {
    // Routed before any Tauri/AppKit init, so neither the version query nor the
    // headless launchd service ever paints a window or claims a Dock icon.
    match lucidos_app::launch_role(std::env::args_os()) {
        LaunchRole::Version => println!("{}", lucidos_app::version_line()),
        LaunchRole::Service => std::process::exit(lucidos_app::run_service()),
        LaunchRole::Client => {
            if let Err(refusal) = lucidos_app::claim_client_owner_lock() {
                eprintln!("{refusal}");
                return;
            }
            lucidos_app::run()
        }
    }
}
