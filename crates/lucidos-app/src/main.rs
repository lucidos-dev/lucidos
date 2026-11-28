use lucidos_app::LaunchRole;

fn main() {
    match lucidos_app::launch_role(std::env::args_os()) {
        LaunchRole::Version => println!("{}", lucidos_app::version_line()),
        LaunchRole::Service => std::process::exit(lucidos_app::run_service()),
        LaunchRole::DisposableService(args) => {
            std::process::exit(lucidos_app::run_disposable_service(args))
        }
        LaunchRole::CaptureClientRelaunchLog => lucidos_app::capture_client_relaunch_log(),
        LaunchRole::RecordClientRelaunch(outcome) => lucidos_app::record_client_relaunch(outcome),
        LaunchRole::Client => {
            if let Err(refusal) = lucidos_app::claim_client_owner_lock() {
                eprintln!("{refusal}");
                return;
            }
            lucidos_app::run()
        }
    }
}
