//! The app's copy of the gateway constants it must agree with (ADR 0368). They
//! live in `lucidos-gateway-constants`, so the engine renders them without
//! depending on the gateway (ADR 0014).

use lucidos_gateway_constants::{PAIRING_CODE_DIGITS, PAIR_PARAM, SIGIL};

use super::{quoted, REGENERATE};

pub(super) fn render() -> String {
    let mut out = String::new();
    out.push_str("// AUTO-GENERATED. Do not edit by hand.\n");
    out.push_str(&format!("// Regenerate: {REGENERATE}\n"));
    out.push_str("//\n");
    out.push_str("// Source of truth: the constant each export names, in\n");
    out.push_str("// crates/lucidos-gateway-constants/src/lib.rs.\n\n");
    out.push_str("/** `PAIRING_CODE_DIGITS` in `lucidos-gateway-constants`. */\n");
    out.push_str(&format!(
        "export const PAIRING_CODE_DIGITS = {PAIRING_CODE_DIGITS};\n\n"
    ));
    out.push_str("/** `PAIR_PARAM` in `lucidos-gateway-constants`. */\n");
    out.push_str(&format!(
        "export const PAIR_PARAM = {};\n\n",
        quoted(PAIR_PARAM)
    ));
    out.push_str("/** `SIGIL` in `lucidos-gateway-constants`. */\n");
    out.push_str(&format!(
        "export const SIGIL = {};\n",
        quoted(&SIGIL.to_string())
    ));
    out
}
