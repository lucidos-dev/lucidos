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

pub const DATA_MUTATION_REPO_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

pub const DATA_MUTATION_RESPONSE_DEADLINE: std::time::Duration =
    std::time::Duration::from_secs(DATA_MUTATION_REPO_WAIT.as_secs() + 40);

pub const WORKSPACE_REPO_BUSY: &str = "workspace_repo_busy";

/// The lowest port a dev launch pins an engine to: `LUCIDOS_ENGINE_PORT_BASE`
/// in `scripts/lib/ports.sh`, which a test reads back (ADR 0821).
pub const DEV_ENGINE_PORT_BASE: u16 = 17173;

/// Where dev engines lived before ADR 0821: `LUCIDOS_LEGACY_ENGINE_PORT_BASE`.
pub const DEV_LEGACY_ENGINE_PORT_BASE: u16 = 5173;

/// How far a dev port walks from its base: `LUCIDOS_PORT_WALK_LIMIT`.
pub const DEV_ENGINE_PORT_WALK_LIMIT: u16 = 1000;

/// The same offset on the new base when `port` lies in the legacy dev range,
/// else `None`. Mirrors `translate_legacy_engine_port` in `ports.sh`.
pub fn translate_legacy_dev_engine_port(port: u16) -> Option<u16> {
    let offset = port.checked_sub(DEV_LEGACY_ENGINE_PORT_BASE)?;
    (offset < DEV_ENGINE_PORT_WALK_LIMIT).then(|| DEV_ENGINE_PORT_BASE + offset)
}

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

    fn ports_sh_value(name: &str) -> u16 {
        let ports_sh =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/lib/ports.sh");
        let script = std::fs::read_to_string(&ports_sh).expect("read scripts/lib/ports.sh");
        let prefix = format!("{name}=");
        script
            .lines()
            .find_map(|line| line.strip_prefix(prefix.as_str()))
            .unwrap_or_else(|| panic!("ports.sh defines {name}"))
            .trim()
            .parse()
            .unwrap_or_else(|e| panic!("{name} is a port number: {e}"))
    }

    #[test]
    fn the_dev_port_constants_match_the_shell_allocator() {
        assert_eq!(
            ports_sh_value("LUCIDOS_ENGINE_PORT_BASE"),
            DEV_ENGINE_PORT_BASE
        );
        assert_eq!(
            ports_sh_value("LUCIDOS_LEGACY_ENGINE_PORT_BASE"),
            DEV_LEGACY_ENGINE_PORT_BASE
        );
        assert_eq!(
            ports_sh_value("LUCIDOS_PORT_WALK_LIMIT"),
            DEV_ENGINE_PORT_WALK_LIMIT
        );
    }

    #[test]
    fn a_legacy_dev_port_keeps_its_offset_on_the_new_base() {
        assert_eq!(translate_legacy_dev_engine_port(5173), Some(17173));
        assert_eq!(translate_legacy_dev_engine_port(5175), Some(17175));
        assert_eq!(translate_legacy_dev_engine_port(6172), Some(18172));
        assert_eq!(translate_legacy_dev_engine_port(6173), None);
        assert_eq!(translate_legacy_dev_engine_port(5172), None);
        assert_eq!(translate_legacy_dev_engine_port(17173), None);
        assert_eq!(translate_legacy_dev_engine_port(52000), None);
    }

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
