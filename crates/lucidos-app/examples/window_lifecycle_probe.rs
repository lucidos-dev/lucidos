#[cfg(target_os = "macos")]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(lucidos_app::window_probe::run(&args));
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("The window lifecycle probe requires macOS.");
    std::process::exit(1);
}
