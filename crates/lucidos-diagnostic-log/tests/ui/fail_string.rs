use lucidos_diagnostic_log::line;
fn main() {
    let value = String::from("conversation marker");
    let _ = line!("value {value:?}");
}
