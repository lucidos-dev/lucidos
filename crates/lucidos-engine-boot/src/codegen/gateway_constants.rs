//! The app's copy of the gateway constants it must agree with (ADR 0368). They
//! live in `lucidos-gateway-constants`, so the engine renders them without
//! depending on the gateway (ADR 0014).

use lucidos_gateway_constants::{
    DEVICE_PAIRING_SECRET_PREFIX, GATEWAY_PAIRING_SECRET_PREFIX, OAUTH_WEB_CALLBACK_PATH,
    PAIRING_CODE_DIGITS, PAIRING_SECRET_CHARS, PAIR_PARAM, REMOTE_GATEWAY_NAME_MAX_LEN,
    REMOTE_ROUTE_SEPARATOR, SIGIL,
};

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
    out.push_str("/** `DEVICE_PAIRING_SECRET_PREFIX` in `lucidos-gateway-constants`. */\n");
    out.push_str(&format!(
        "export const DEVICE_PAIRING_SECRET_PREFIX = {};\n\n",
        quoted(DEVICE_PAIRING_SECRET_PREFIX)
    ));
    out.push_str("/** `GATEWAY_PAIRING_SECRET_PREFIX` in `lucidos-gateway-constants`. */\n");
    out.push_str(&format!(
        "export const GATEWAY_PAIRING_SECRET_PREFIX = {};\n\n",
        quoted(GATEWAY_PAIRING_SECRET_PREFIX)
    ));
    out.push_str("/** `PAIRING_SECRET_CHARS` in `lucidos-gateway-constants`. */\n");
    out.push_str(&format!(
        "export const PAIRING_SECRET_CHARS = {PAIRING_SECRET_CHARS};\n\n"
    ));
    out.push_str("/** `PAIR_PARAM` in `lucidos-gateway-constants`. */\n");
    out.push_str(&format!(
        "export const PAIR_PARAM = {};\n\n",
        quoted(PAIR_PARAM)
    ));
    out.push_str("/** `REMOTE_GATEWAY_NAME_MAX_LEN` in `lucidos-gateway-constants`. */\n");
    out.push_str(&format!(
        "export const REMOTE_GATEWAY_NAME_MAX_LEN = {REMOTE_GATEWAY_NAME_MAX_LEN};\n\n"
    ));
    out.push_str("/** `REMOTE_ROUTE_SEPARATOR` in `lucidos-gateway-constants`. */\n");
    out.push_str(&format!(
        "export const REMOTE_ROUTE_SEPARATOR = {};\n\n",
        quoted(&REMOTE_ROUTE_SEPARATOR.to_string())
    ));
    out.push_str("/** `OAUTH_WEB_CALLBACK_PATH` in `lucidos-gateway-constants`. */\n");
    out.push_str(&format!(
        "export const OAUTH_WEB_CALLBACK_PATH = {};\n\n",
        quoted(OAUTH_WEB_CALLBACK_PATH)
    ));
    out.push_str("/** `SIGIL` in `lucidos-gateway-constants`. */\n");
    out.push_str(&format!(
        "export const SIGIL = {};\n",
        quoted(&SIGIL.to_string())
    ));
    out
}
