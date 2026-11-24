use lucidos_diagnostic_log::line;
fn main() {
    let value = "conversation marker";
    let _ = line!("value {value:?}");
}
