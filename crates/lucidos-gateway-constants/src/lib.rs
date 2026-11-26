pub const LOG_STAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%:z";

pub const DEVICE_SESSION_OWNER_DEVICE: &str = "device_session_owner_device";

pub const PAIRING_CODE_DIGITS: u32 = 8;

pub const DEVICE_PAIRING_SECRET_PREFIX: &str = "ldp_";

pub const GATEWAY_PAIRING_SECRET_PREFIX: &str = "lgp_";

pub const PAIRING_SECRET_CHARS: usize = 26;

pub const PAIR_PARAM: &str = "pair";

pub const SIGIL: char = '~';

pub const GATEWAY_HEALTH_PATH: &str = "/~/api/v1/health";

pub const REMOTE_ROUTE_SEPARATOR: char = '@';

pub const REMOTE_GATEWAY_NAME_MAX_LEN: usize = 32;

pub const OAUTH_CALLBACK_PORT: u16 = 14981;

pub const OAUTH_CALLBACK_PATH: &str = "/oauth/callback";

pub const OAUTH_STATE_PREFIX_SEPARATOR: char = '~';

pub const OAUTH_DELIVERY_ROUTE: &str = "/internal/oauth/callback";

pub const OAUTH_WEB_CALLBACK_PATH: &str = "/~/oauth/callback";

pub const HEADER_LOCAL_BROWSER: &str = "x-lucidos-local-browser";

pub const WORKSPACE_DATABASE_PREFIX: &str = "lucidos_";

pub fn is_valid_workspace_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        && !id.starts_with('-')
        && !id.ends_with('-')
}

pub fn display_names_match(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

pub fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "workspace".to_string()
    } else {
        out
    }
}

pub fn workspace_database_name(
    id: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    if !is_valid_workspace_id(id) {
        return Err(format!("invalid workspace id for database name: '{id}'").into());
    }
    Ok(format!("{WORKSPACE_DATABASE_PREFIX}{id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_slug_contract_and_database_mapping_are_shared() {
        for slug in ["demo", "e2e-test", "a1", &"a".repeat(64)] {
            assert!(is_valid_workspace_id(slug));
            assert_eq!(
                workspace_database_name(slug).unwrap(),
                format!("lucidos_{slug}")
            );
        }
        for slug in [
            "",
            "../demo",
            "Demo",
            "demo@remote",
            "-demo",
            "demo-",
            &"a".repeat(65),
        ] {
            assert!(!is_valid_workspace_id(slug));
            assert!(workspace_database_name(slug).is_err());
        }
    }
}

pub mod backup_archive;
