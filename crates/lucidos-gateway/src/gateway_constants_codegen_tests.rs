//! The app's copy of the gateway constants it must agree with. Generated, so
//! the app never restates one (ADR 0368). The gateway does not depend on the
//! engine, so this is its own generator.

use std::path::PathBuf;

use crate::auth::PAIRING_CODE_DIGITS;
use crate::pairing_qr::PAIR_PARAM;
use crate::registry::SIGIL;

const REGENERATE: &str =
    "cargo test -p lucidos-gateway generate_gateway_constants_file -- --ignored";

fn ts_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../lucidos-app/src/generated/gateway-constants.ts")
}

fn quoted(text: &str) -> String {
    serde_json::to_string(text).expect("a string serialises")
}

fn generate_ts() -> String {
    let mut out = String::new();
    out.push_str("// AUTO-GENERATED. Do not edit by hand.\n");
    out.push_str(&format!("// Regenerate: {REGENERATE}\n"));
    out.push_str("//\n");
    out.push_str("// Source of truth: the gateway constant each export names, under\n");
    out.push_str("// crates/lucidos-gateway/src.\n\n");
    out.push_str("/** `PAIRING_CODE_DIGITS` in `auth.rs`. */\n");
    out.push_str(&format!(
        "export const PAIRING_CODE_DIGITS = {PAIRING_CODE_DIGITS};\n\n"
    ));
    out.push_str("/** `PAIR_PARAM` in `pairing_qr.rs`. */\n");
    out.push_str(&format!(
        "export const PAIR_PARAM = {};\n\n",
        quoted(PAIR_PARAM)
    ));
    out.push_str("/** `SIGIL` in `registry.rs`. */\n");
    out.push_str(&format!(
        "export const SIGIL = {};\n",
        quoted(&SIGIL.to_string())
    ));
    out
}

#[test]
fn generated_gateway_constants_are_up_to_date() {
    let path = ts_path();
    match std::fs::read_to_string(&path) {
        Ok(existing) => assert_eq!(
            existing,
            generate_ts(),
            "Generated {} is stale. Run: {REGENERATE}",
            path.display()
        ),
        Err(_) => panic!(
            "Generated file missing at {}. Run: {REGENERATE}",
            path.display()
        ),
    }
}

#[test]
#[ignore]
fn generate_gateway_constants_file() {
    let path = ts_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, generate_ts()).unwrap();
}
