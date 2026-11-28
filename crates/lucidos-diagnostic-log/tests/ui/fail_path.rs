use lucidos_diagnostic_log::line;
fn main() {
    let value = std::path::PathBuf::from("/home/user/private");
    let _ = line!("value {value:?}");
}
