//! The desktop window's own way in.
//!
//! The gateway answers no network caller it has not paired (ADR 0094), and the
//! desktop window is `WebviewUrl::External` pointed at it. So the window is an
//! ordinary browser, with no credential, exactly like a phone.
//!
//! What it has that a phone does not is a Rust side on the same machine, which
//! can read the mode 0600 local token. That makes it a pairing authority. This
//! module is the one command that uses it: mint a code, hand it to the page.
//!
//! # The page redeems, this does not
//!
//! Redeeming here would put the credential in a Rust HTTP client, which
//! authorizes nothing the user can see. The cookie has to land in the webview's
//! own jar, so the page posts the code to `/~/api/v1/auth/pair` itself.
//!
//! The token never crosses IPC. Only the code does, and a code is single use
//! and expires in five minutes.

use serde::Serialize;

/// Where the gateway mints a pairing code, behind the reserved sigil namespace.
const PAIRING_CODE_PATH: &str = "/~/api/v1/auth/pairing-code";

/// A minted code and how long the page has to spend it.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct MintedCode {
    pub code: String,
    pub expires_in_secs: u64,
}

/// Mint a one-time pairing code for the window that asked.
///
/// `Err` whenever the gateway did not answer with a code. The page reads that
/// as "fall back to the typed form", never as a dead end.
///
/// # What registering this hands the gateway origin
///
/// `desktop.rs`'s `GATEWAY_PERMISSIONS` header asks that of every new entry.
/// `allow-app-ipc` grants each registered command on that origin, so this one
/// lets whoever answers on the gateway port pair a device in. That same grant
/// already carries `updater:default`, a signed bundle swap and a stack restart.
/// Pairing is strictly smaller, and ADR 0028 accepts the larger residual.
#[tauri::command]
pub fn mint_pairing_code() -> Result<MintedCode, String> {
    let port = crate::desktop::engine_port();
    let body = crate::desktop::gateway_body(port, "POST", PAIRING_CODE_PATH)
        .ok_or_else(|| format!("the gateway on port {port} did not mint a pairing code"))?;
    parse_minted_code(&body).ok_or_else(|| "the gateway returned no pairing code".to_string())
}

/// Pure: read the mint response, so the shape is tested without a gateway.
///
/// A body that parses but carries no usable code is a miss. Otherwise the page
/// would be handed an empty code and post it.
fn parse_minted_code(body: &str) -> Option<MintedCode> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let code = json.get("code")?.as_str()?.trim().to_string();
    if code.is_empty() {
        return None;
    }
    Some(MintedCode {
        code,
        // The gateway has always sent this. A default keeps an older one
        // working rather than failing the pair over a countdown.
        expires_in_secs: json
            .get("expires_in_secs")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(300),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minted_code_is_read_with_its_expiry() {
        let body = r#"{"code":"12345678","expires_in_secs":300}"#;
        assert_eq!(
            parse_minted_code(body),
            Some(MintedCode {
                code: "12345678".into(),
                expires_in_secs: 300
            })
        );
    }

    #[test]
    fn the_qr_fields_are_ignored_rather_than_required() {
        // The page asks for no origin, so the gateway omits `pair_url` and
        // `qr_svg`. A future field must not break the read either.
        let body = r#"{"code":"87654321","expires_in_secs":300,"pair_url":"x","qr_svg":"y"}"#;
        assert_eq!(parse_minted_code(body).unwrap().code, "87654321");
    }

    #[test]
    fn a_body_with_no_usable_code_is_a_miss() {
        assert!(parse_minted_code("not json").is_none());
        assert!(parse_minted_code("{}").is_none());
        assert!(parse_minted_code(r#"{"code":""}"#).is_none());
        assert!(parse_minted_code(r#"{"code":"   "}"#).is_none());
        assert!(parse_minted_code(r#"{"code":12345678}"#).is_none());
    }

    #[test]
    fn a_missing_expiry_falls_back_rather_than_failing_the_pair() {
        let minted = parse_minted_code(r#"{"code":"12345678"}"#).unwrap();
        assert_eq!(minted.expires_in_secs, 300);
    }

    #[test]
    fn the_minted_code_carries_no_local_token() {
        // The token is a pairing authority (`docs/glossary.md` § Local token).
        // What crosses IPC is the code, which is single use and expires.
        let minted = parse_minted_code(r#"{"code":"12345678","expires_in_secs":300}"#).unwrap();
        let wire = serde_json::to_string(&minted).unwrap();
        assert_eq!(wire, r#"{"code":"12345678","expires_in_secs":300}"#);
    }
}
