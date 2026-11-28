use lucidos_diagnostic_log::{line, private};
fn main() {
    lucidos_diagnostic_log::log!("plain {id}", id = 42_u64);
    let path = std::path::PathBuf::from("/home/user/private");
    let _ = line!("{:?}", private(path));
    let text = String::from("conversation marker");
    let _ = line!("{} {flag} {id} {elapsed:?} {stamp} {status} {kind:?} {}",
        private(&text), 42_u64,
        flag = true,
        id = uuid::Uuid::nil(),
        elapsed = std::time::Duration::from_millis(7),
        stamp = chrono::Utc::now(),
        status = http::StatusCode::BAD_GATEWAY,
        kind = std::io::ErrorKind::NotFound);
    let _ = line!("{{escaped}} {:08x} {value:.precision$}", 31_u32,
        value = 1.25_f64, precision = 1_usize);
}
