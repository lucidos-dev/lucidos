use lucidos_diagnostic_log::line;
fn main() {
    let value = std::io::Error::other("private error").to_string();
    let _ = line!("value {value:?}");
}
