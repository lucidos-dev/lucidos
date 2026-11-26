use serde::Serialize;

const PAIRING_CODE_PATH: &str = "/~/api/v1/auth/pairing-code";

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct MintedCode {
    pub code: String,
    pub expires_in_secs: u64,
}

#[tauri::command]
pub fn mint_pairing_code() -> Result<MintedCode, String> {
    let port = crate::desktop::engine_port();
    let body = crate::desktop::gateway_body(port, "POST", PAIRING_CODE_PATH, None)
        .ok_or_else(|| format!("the gateway on port {port} did not mint a pairing code"))?;
    parse_minted_code(&body).ok_or_else(|| "the gateway returned no pairing code".to_string())
}

fn parse_minted_code(body: &str) -> Option<MintedCode> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let code = json
        .get("secret")
        .or_else(|| json.get("code"))?
        .as_str()?
        .trim()
        .to_string();
    if code.is_empty() {
        return None;
    }
    for key in ["secret", "code"] {
        if let Some(value) = json.get(key).and_then(serde_json::Value::as_str) {
            crate::client_log::secret(value.trim().to_string());
        }
    }
    Some(MintedCode {
        code,
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
    fn both_normalized_pairing_credentials_are_registered_before_return() {
        let minted =
            parse_minted_code(r#"{"code":" 31415926 ","secret":" opaque-shell-pairing-fixture "}"#)
                .unwrap();
        assert_eq!(minted.code, "opaque-shell-pairing-fixture");
        let text = lucidos_diagnostic_log::sanitize::sanitize(
            "pairing 31415926 opaque-shell-pairing-fixture",
        );
        assert!(!text.contains("31415926"));
        assert!(!text.contains("opaque-shell-pairing-fixture"));
    }

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
    fn the_long_secret_is_preferred_over_the_short_code() {
        let body = r#"{"code":"12345678","secret":"ldp_abcdefghijklmnopqrstuvwxyz","expires_in_secs":300}"#;
        assert_eq!(
            parse_minted_code(body).map(|m| m.code).as_deref(),
            Some("ldp_abcdefghijklmnopqrstuvwxyz")
        );
    }

    #[test]
    fn the_qr_fields_are_ignored_rather_than_required() {
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
        let minted = parse_minted_code(r#"{"code":"12345678","expires_in_secs":300}"#).unwrap();
        let wire = serde_json::to_string(&minted).unwrap();
        assert_eq!(wire, r#"{"code":"12345678","expires_in_secs":300}"#);
    }
}
