use axum::http::HeaderMap;
use lucidos_local_token as local_token;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub use local_token::{
    ct_eq, path as local_token_path, HEADER_HOOK_SOCKET_TOKEN, HEADER_LOCAL_TOKEN,
};

pub fn sensitive_credential(value: &str) -> Option<axum::http::HeaderValue> {
    let mut header = axum::http::HeaderValue::from_str(value).ok()?;
    header.set_sensitive(true);
    Some(header)
}

pub const DEVICE_COOKIE_STEM: &str = "lucidos_device";

pub fn device_cookie_name(app_data: &Path) -> String {
    let id = &digest(&app_data.to_string_lossy())[..8];
    format!("{DEVICE_COOKIE_STEM}_{id}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentedCredential<'a> {
    value: &'a str,
    own_name: bool,
}

impl<'a> PresentedCredential<'a> {
    pub fn value(self) -> &'a str {
        self.value
    }

    pub fn is_renamed(self) -> bool {
        !self.own_name
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Authorization {
    LocalProcess,
    Device { id: String, label: String },
    Unauthorized,
}

#[derive(Debug, Clone, Copy)]
pub struct AuthenticatedLocalProcess;

pub const HEADER_AGENT_ORIGIN_TOKEN: &str = "x-lucidos-agent-origin-token";

#[derive(Debug, Clone, Copy)]
pub struct AgentRestoreCaller;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedDevice {
    pub id: String,
    pub label: String,
}

impl AuthenticatedDevice {
    pub fn forwarded_headers(
        &self,
        workspace_id: &str,
        stamp_key: Option<&lucidos_gateway_stamp::StampKey>,
    ) -> Vec<(&'static str, String)> {
        let mut headers = vec![
            (crate::stack::HEADER_DEVICE_ID, self.id.clone()),
            (
                crate::stack::HEADER_DEVICE_LABEL,
                urlencoding::encode(&self.label).into_owned(),
            ),
        ];
        if let Some(key) = stamp_key {
            headers.push((
                lucidos_gateway_stamp::HEADER_GATEWAY_STAMP,
                key.sign_now(lucidos_gateway_stamp::Claim::Device {
                    workspace_id,
                    device_id: &self.id,
                }),
            ));
        }
        headers
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairedDevice {
    pub id: String,
    pub label: String,
    pub credential_digest: String,
    pub paired_at: String,
    #[serde(default)]
    pub last_seen_at: Option<String>,
}

pub const LAST_SEEN_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

pub fn last_seen_is_due(last_seen_at: Option<&str>, now: chrono::DateTime<chrono::Utc>) -> bool {
    let Some(raw) = last_seen_at else {
        return true;
    };
    let Ok(seen) = chrono::DateTime::parse_from_rfc3339(raw) else {
        return true;
    };
    now.signed_duration_since(seen.with_timezone(&chrono::Utc))
        .to_std()
        .is_ok_and(|elapsed| elapsed >= LAST_SEEN_INTERVAL)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairedDevices {
    #[serde(default)]
    pub devices: Vec<PairedDevice>,
}

pub fn load_store<T: Default + serde::de::DeserializeOwned>(
    path: &Path,
) -> Result<T, crate::BoxError> {
    match std::fs::read_to_string(path) {
        Ok(raw) if raw.trim().is_empty() => Ok(T::default()),
        Ok(raw) => Ok(serde_json::from_str(&raw)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(Box::new(e)),
    }
}

pub fn save_store<T: Serialize>(store: &T, path: &Path) -> Result<(), crate::BoxError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    local_token::write_owner_only(&tmp, &serde_json::to_string_pretty(store)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

impl PairedDevices {
    pub fn load(path: &Path) -> Result<Self, crate::BoxError> {
        load_store(path)
    }

    #[cfg(test)]
    pub fn save(&self, path: &Path) -> Result<(), crate::BoxError> {
        save_store(self, path)
    }

    fn match_credential(&self, credential: &str) -> Option<&PairedDevice> {
        let digest = digest(credential);
        self.devices
            .iter()
            .find(|d| ct_eq(&d.credential_digest, &digest))
    }
}

pub fn paired_devices_path(app_data: &Path) -> PathBuf {
    app_data.join(lucidos_home::private::PAIRED_DEVICES)
}

pub fn legacy_paired_devices_path() -> Option<PathBuf> {
    lucidos_home::private_entry(lucidos_home::private::PAIRED_DEVICES)
}

pub fn digest(value: &str) -> String {
    use sha2::{Digest as _, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    let out = hasher.finalize();
    let mut s = String::with_capacity(out.len() * 2);
    for b in out {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

pub const REFUSAL_LOG_WINDOW: Duration = Duration::from_secs(60);

pub fn refusal_is_worth_logging(audit: CookieAudit) -> bool {
    audit.cookies > 0
}

pub fn refusal_line(audit: CookieAudit, cookie_name: &str, devices: usize) -> String {
    let CookieAudit {
        fields,
        cookies,
        credentials,
        own_name,
    } = audit;
    format!(
        "[Gateway] refused a device: {cookies} cookie(s) over {fields} cookie field(s), of which \
         {credentials} credential(s), {} under this gateway's own name {cookie_name}, and none \
         matched any of the {devices} paired device(s)",
        if own_name { "one" } else { "none" },
    )
}

#[derive(Debug, Default)]
pub struct LogThrottle {
    opened: Option<Instant>,
}

impl LogThrottle {
    pub fn allow(&mut self, now: Instant, window: Duration) -> bool {
        match self.opened {
            Some(opened) if now.duration_since(opened) < window => false,
            _ => {
                self.opened = Some(now);
                true
            }
        }
    }
}

pub fn mint_credential() -> std::io::Result<String> {
    local_token::mint_hex(32).map(crate::diagnostic_log::secret)
}

const HARDENED_PREFIX: &str = "__Host-";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CookieForm {
    Hardened,
    Plain,
}

impl CookieForm {
    pub fn of(headers: &HeaderMap, connection: &lucidos_gateway_tls::Connection) -> CookieForm {
        let forwarded_https = headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.split(',').next().is_some_and(|p| p.trim() == "https"));
        if connection.is_tls() || forwarded_https {
            CookieForm::Hardened
        } else {
            CookieForm::Plain
        }
    }

    pub fn of_request<B>(req: &axum::http::Request<B>) -> CookieForm {
        CookieForm::of(
            req.headers(),
            &lucidos_gateway_tls::Connection::of(req.extensions()),
        )
    }

    pub fn name(self, base: &str) -> String {
        match self {
            CookieForm::Hardened => format!("{HARDENED_PREFIX}{base}"),
            CookieForm::Plain => base.to_string(),
        }
    }

    fn base_of(self, name: &str) -> Option<&str> {
        match self {
            CookieForm::Hardened => name.strip_prefix(HARDENED_PREFIX),
            CookieForm::Plain => Some(name),
        }
    }

    fn attributes(self) -> &'static str {
        match self {
            CookieForm::Hardened => "; Secure; Partitioned",
            CookieForm::Plain => "",
        }
    }
}

pub fn credential_cookie(base: &str, credential: &str, form: CookieForm) -> String {
    format!(
        "{}={credential}; Path=/; HttpOnly; SameSite=Lax; Max-Age=31536000{}",
        form.name(base),
        form.attributes()
    )
}

pub fn cleared_credential_cookie(base: &str, form: CookieForm) -> String {
    format!(
        "{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{}",
        form.name(base),
        form.attributes()
    )
}

pub fn ensure_local_token() -> std::io::Result<String> {
    local_token::ensure().map(crate::diagnostic_log::secret)
}

pub fn ensure_hook_socket_token() -> std::io::Result<String> {
    local_token::ensure_named(local_token::HOOK_SOCKET_TOKEN_FILE)
        .map(crate::diagnostic_log::secret)
}

pub fn mint_device_id() -> std::io::Result<String> {
    local_token::mint_hex(8)
}

pub fn authorize_with_match<'a>(
    headers: &'a HeaderMap,
    local_token: &str,
    paired: &PairedDevices,
    cookie_name: &str,
    form: CookieForm,
) -> (Authorization, Option<PresentedCredential<'a>>) {
    if presented_local_token(headers).is_some_and(|t| ct_eq(t, local_token)) {
        return (Authorization::LocalProcess, None);
    }
    for presented in presented_credentials(headers, cookie_name, form) {
        if let Some(device) = paired.match_credential(presented.value()) {
            return (
                Authorization::Device {
                    id: device.id.clone(),
                    label: device.label.clone(),
                },
                Some(presented),
            );
        }
    }
    (Authorization::Unauthorized, None)
}

fn presented_local_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(HEADER_LOCAL_TOKEN)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

pub fn presented_credentials<'a>(
    headers: &'a HeaderMap,
    cookie_name: &str,
    form: CookieForm,
) -> Vec<PresentedCredential<'a>> {
    let mut found = Vec::new();
    for (name, value) in cookie_pairs(headers) {
        let Some(base) = form.base_of(name).filter(|b| is_device_cookie_name(b)) else {
            continue;
        };
        let own_name = base == cookie_name;
        let presented = PresentedCredential { value, own_name };
        if own_name {
            found.insert(0, presented);
        } else {
            found.push(presented);
        }
    }
    found
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CookieAudit {
    pub fields: usize,
    pub cookies: usize,
    pub credentials: usize,
    pub own_name: bool,
}

pub fn presented_credential_summary(
    headers: &HeaderMap,
    cookie_name: &str,
    form: CookieForm,
) -> CookieAudit {
    let mut audit = CookieAudit {
        fields: headers.get_all(axum::http::header::COOKIE).iter().count(),
        ..CookieAudit::default()
    };
    for (name, _) in cookie_pairs(headers) {
        audit.cookies += 1;
        let Some(base) = form.base_of(name).filter(|b| is_device_cookie_name(b)) else {
            continue;
        };
        audit.credentials += 1;
        audit.own_name |= base == cookie_name;
    }
    audit
}

fn is_device_cookie_name(name: &str) -> bool {
    match name.strip_prefix(DEVICE_COOKIE_STEM) {
        Some("") => true,
        Some(rest) => rest.starts_with('_') && rest.len() > 1,
        None => false,
    }
}

pub(crate) fn cookie_pairs(headers: &HeaderMap) -> impl Iterator<Item = (&str, &str)> {
    headers
        .get_all(axum::http::header::COOKIE)
        .into_iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|raw| raw.split(';'))
        .filter_map(|pair| pair.split_once('='))
        .map(|(name, value)| (name.trim(), value.trim()))
        .filter(|(_, value)| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                axum::http::HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    fn cookie_fields(fields: &[&str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for field in fields {
            h.append(
                axum::http::header::COOKIE,
                axum::http::HeaderValue::from_str(field).unwrap(),
            );
        }
        h
    }

    const OWN_COOKIE: &str = "lucidos_device_deadbeef";

    fn authorize(
        headers: &HeaderMap,
        local_token: &str,
        paired: &PairedDevices,
        cookie_name: &str,
    ) -> Authorization {
        authorize_with_match(headers, local_token, paired, cookie_name, CookieForm::Plain).0
    }

    fn paired_with(credential: &str) -> PairedDevices {
        PairedDevices {
            devices: vec![PairedDevice {
                id: "device-1".into(),
                label: "My iPhone".into(),
                credential_digest: digest(credential),
                paired_at: "2026-08-17T00:00:00Z".into(),
                last_seen_at: None,
            }],
        }
    }

    #[test]
    fn no_credential_is_unauthorized() {
        let none = PairedDevices::default();
        assert_eq!(
            authorize(&headers(&[]), "secret", &none, OWN_COOKIE),
            Authorization::Unauthorized
        );
    }

    #[test]
    fn the_local_token_proves_a_local_process() {
        let none = PairedDevices::default();
        let h = headers(&[(HEADER_LOCAL_TOKEN, "secret")]);
        assert_eq!(
            authorize(&h, "secret", &none, OWN_COOKIE),
            Authorization::LocalProcess
        );
    }

    #[test]
    fn a_wrong_local_token_is_unauthorized() {
        let none = PairedDevices::default();
        let h = headers(&[(HEADER_LOCAL_TOKEN, "nope")]);
        assert_eq!(
            authorize(&h, "secret", &none, OWN_COOKIE),
            Authorization::Unauthorized
        );
    }

    #[test]
    fn a_blank_local_token_is_unauthorized() {
        let none = PairedDevices::default();
        let h = headers(&[(HEADER_LOCAL_TOKEN, "   ")]);
        assert_eq!(
            authorize(&h, "secret", &none, OWN_COOKIE),
            Authorization::Unauthorized
        );
        assert_eq!(
            authorize(&h, "", &none, OWN_COOKIE),
            Authorization::Unauthorized
        );
    }

    #[test]
    fn a_device_id_header_authorizes_nothing() {
        let none = PairedDevices::default();
        let h = headers(&[("x-lucidos-device-id", "some-real-device-id")]);
        assert_eq!(
            authorize(&h, "secret", &none, OWN_COOKIE),
            Authorization::Unauthorized
        );
    }

    #[test]
    fn a_paired_credential_cookie_names_its_device() {
        let paired = paired_with("cred-abc");
        let h = headers(&[("cookie", "lucidos_device=cred-abc")]);
        assert_eq!(
            authorize(&h, "secret", &paired, OWN_COOKIE),
            Authorization::Device {
                id: "device-1".into(),
                label: "My iPhone".into()
            }
        );
    }

    #[test]
    fn an_unknown_credential_cookie_is_unauthorized() {
        let paired = paired_with("cred-abc");
        let h = headers(&[("cookie", "lucidos_device=cred-xyz")]);
        assert_eq!(
            authorize(&h, "secret", &paired, OWN_COOKIE),
            Authorization::Unauthorized
        );
    }

    #[test]
    fn the_cookie_is_found_among_siblings_and_matched_by_exact_name() {
        let paired = paired_with("cred-abc");
        let h = headers(&[("cookie", "theme=dark; lucidos_device=cred-abc; other=1")]);
        assert!(matches!(
            authorize(&h, "secret", &paired, OWN_COOKIE),
            Authorization::Device { .. }
        ));

        let h = headers(&[("cookie", "xlucidos_device=cred-abc")]);
        assert_eq!(
            authorize(&h, "secret", &paired, OWN_COOKIE),
            Authorization::Unauthorized
        );
    }

    #[test]
    fn two_data_dirs_get_two_cookie_names() {
        let a = device_cookie_name(Path::new("/tmp/gw-a"));
        let b = device_cookie_name(Path::new("/tmp/gw-b"));
        assert_ne!(a, b);
        assert!(a.starts_with("lucidos_device_"), "{a}");
        assert_eq!(a, device_cookie_name(Path::new("/tmp/gw-a")));
        assert_ne!(a, DEVICE_COOKIE_STEM);
    }

    #[test]
    fn this_gateways_own_cookie_is_tried_first() {
        let own = device_cookie_name(Path::new("/tmp/gw-a"));
        let h = headers(&[(
            "cookie",
            &format!("lucidos_device=legacy-value; {own}=own-value"),
        )]);
        let candidates = presented_credentials(&h, &own, CookieForm::Plain);
        assert_eq!(candidates[0].value(), "own-value");
        assert!(!candidates[0].is_renamed());
        assert_eq!(candidates.len(), 2);
    }

    #[test]
    fn a_pre_split_cookie_is_read_when_this_gateway_has_none_yet() {
        let own = device_cookie_name(Path::new("/tmp/gw-a"));
        let h = headers(&[("cookie", "lucidos_device=legacy-value")]);
        let candidates = presented_credentials(&h, &own, CookieForm::Plain);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].is_renamed());
        assert_eq!(candidates[0].value(), "legacy-value");
    }

    #[test]
    fn a_stale_own_cookie_never_masks_a_credential_the_store_holds() {
        let own = device_cookie_name(Path::new("/tmp/gw-a"));
        let paired = paired_with("cred-abc");
        let h = headers(&[(
            "cookie",
            &format!("{own}=long-dead; lucidos_device=cred-abc"),
        )]);
        let (decision, matched) =
            authorize_with_match(&h, "secret", &paired, &own, CookieForm::Plain);
        assert!(matches!(decision, Authorization::Device { .. }));
        let matched = matched.expect("the match is handed back for the re-issue");
        assert_eq!(matched.value(), "cred-abc");
        assert!(matched.is_renamed(), "it must be moved to our own name");
    }

    #[test]
    fn a_cookie_left_by_a_data_dir_move_still_authorizes() {
        let before = device_cookie_name(Path::new("/tmp/gw-old"));
        let after = device_cookie_name(Path::new("/tmp/gw-new"));
        let paired = paired_with("cred-abc");
        let h = headers(&[("cookie", &format!("{before}=cred-abc"))]);
        let (decision, matched) =
            authorize_with_match(&h, "secret", &paired, &after, CookieForm::Plain);
        assert!(matches!(decision, Authorization::Device { .. }));
        assert!(matched.expect("matched").is_renamed());
    }

    #[test]
    fn another_gateways_cookie_authorizes_nothing_it_does_not_match() {
        let a = device_cookie_name(Path::new("/tmp/gw-a"));
        let b = device_cookie_name(Path::new("/tmp/gw-b"));
        let paired = paired_with("cred-abc");
        let h = headers(&[("cookie", &format!("{b}=b-value"))]);
        assert_eq!(
            authorize(&h, "secret", &paired, &a),
            Authorization::Unauthorized
        );
    }

    #[test]
    fn a_credential_in_a_second_cookie_field_still_authorizes() {
        let own = device_cookie_name(Path::new("/tmp/gw-a"));
        let paired = paired_with("cred-abc");
        let h = cookie_fields(&["theme=dark", &format!("{own}=cred-abc")]);
        let (decision, matched) =
            authorize_with_match(&h, "secret", &paired, &own, CookieForm::Plain);
        assert!(
            matches!(decision, Authorization::Device { .. }),
            "a credential past the first field is still one the client sent"
        );
        assert!(
            !matched.expect("matched").is_renamed(),
            "it arrived under our own name, so nothing is being migrated"
        );
    }

    #[test]
    fn our_own_name_is_tried_first_across_fields_too() {
        let own = device_cookie_name(Path::new("/tmp/gw-a"));
        let h = cookie_fields(&["lucidos_device=legacy-value", &format!("{own}=own-value")]);
        let candidates = presented_credentials(&h, &own, CookieForm::Plain);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].value(), "own-value");
        assert!(!candidates[0].is_renamed());
    }

    #[test]
    fn the_refusal_summary_counts_every_field() {
        let own = device_cookie_name(Path::new("/tmp/gw-a"));
        let h = cookie_fields(&["theme=dark", &format!("{own}=cred-abc")]);
        assert_eq!(
            presented_credential_summary(&h, &own, CookieForm::Plain),
            CookieAudit {
                fields: 2,
                cookies: 2,
                credentials: 1,
                own_name: true,
            }
        );
    }

    #[test]
    fn a_refusal_carrying_cookies_but_no_credential_is_logged() {
        let own = device_cookie_name(Path::new("/tmp/gw-a"));
        let known = presented_credential_summary(
            &headers(&[("cookie", "theme=dark")]),
            &own,
            CookieForm::Plain,
        );
        assert_eq!(known.cookies, 1);
        assert_eq!(known.credentials, 0);
        assert!(refusal_is_worth_logging(known));

        let fresh = presented_credential_summary(&headers(&[]), &own, CookieForm::Plain);
        assert_eq!(fresh, CookieAudit::default());
        assert!(!refusal_is_worth_logging(fresh));
    }

    #[test]
    fn only_the_device_cookie_family_is_read_as_a_credential() {
        let own = device_cookie_name(Path::new("/tmp/gw-a"));
        let paired = paired_with("cred-abc");
        for name in [
            "xlucidos_device",
            "lucidos_devices",
            "lucidos_device_",
            "lucidos_devicex",
            "lucidos",
        ] {
            let h = headers(&[("cookie", &format!("{name}=cred-abc"))]);
            assert_eq!(
                authorize(&h, "secret", &paired, &own),
                Authorization::Unauthorized,
                "{name} was read as a credential cookie"
            );
        }
    }

    #[test]
    fn a_refusal_summary_counts_names_and_carries_no_value() {
        let own = device_cookie_name(Path::new("/tmp/gw-a"));
        let other = device_cookie_name(Path::new("/tmp/gw-b"));
        let h = headers(&[(
            "cookie",
            &format!("theme=dark; {own}=v1; {other}=v2; lucidos_device=v3"),
        )]);
        let audit = presented_credential_summary(&h, &own, CookieForm::Plain);
        assert_eq!(audit.credentials, 3);
        assert_eq!(audit.cookies, 4);
        assert!(audit.own_name);

        let h = headers(&[("cookie", "theme=dark")]);
        let audit = presented_credential_summary(&h, &own, CookieForm::Plain);
        assert_eq!(audit.credentials, 0);
        assert!(!audit.own_name);
    }

    #[test]
    fn a_refusal_line_names_the_door_and_never_a_credential() {
        let audit = CookieAudit {
            fields: 2,
            cookies: 5,
            credentials: 2,
            own_name: true,
        };
        let line = refusal_line(audit, OWN_COOKIE, 3);
        assert!(line.contains(OWN_COOKIE), "{line}");
        assert!(line.contains("2 cookie field(s)"), "{line}");
        assert!(line.contains("5 cookie(s)"), "{line}");
        assert!(line.contains("2 credential(s)"), "{line}");
        assert!(line.contains("3 paired device(s)"), "{line}");
        assert!(line.contains("one under"), "{line}");

        let none_ours = CookieAudit {
            own_name: false,
            ..audit
        };
        assert!(refusal_line(none_ours, OWN_COOKIE, 0).contains("none under"));
    }

    #[test]
    fn a_hot_path_log_line_is_written_once_per_window() {
        let mut throttle = LogThrottle::default();
        let start = Instant::now();
        assert!(throttle.allow(start, REFUSAL_LOG_WINDOW));
        assert!(!throttle.allow(start + Duration::from_secs(1), REFUSAL_LOG_WINDOW));
        assert!(!throttle.allow(
            start + REFUSAL_LOG_WINDOW - Duration::from_millis(1),
            REFUSAL_LOG_WINDOW
        ));
        assert!(throttle.allow(start + REFUSAL_LOG_WINDOW, REFUSAL_LOG_WINDOW));
    }

    #[test]
    fn the_plain_cookie_is_httponly_and_lax_and_never_secure() {
        let cookie = credential_cookie(OWN_COOKIE, "abc", CookieForm::Plain);
        assert_eq!(
            cookie,
            format!("{OWN_COOKIE}=abc; Path=/; HttpOnly; SameSite=Lax; Max-Age=31536000")
        );
    }

    #[test]
    fn the_hardened_cookie_is_host_bound_secure_and_partitioned() {
        let expected = format!(
            "__Host-{OWN_COOKIE}=abc; Path=/; HttpOnly; SameSite=Lax; Max-Age=31536000; \
             Secure; Partitioned"
        );
        assert_eq!(
            credential_cookie(OWN_COOKIE, "abc", CookieForm::Hardened),
            expected
        );
        assert_eq!(
            cleared_credential_cookie(OWN_COOKIE, CookieForm::Hardened),
            format!("__Host-{OWN_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0; Secure; Partitioned")
        );
        assert_eq!(
            cleared_credential_cookie(OWN_COOKIE, CookieForm::Plain),
            format!("{OWN_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
        );
    }

    #[test]
    fn the_form_follows_our_own_tls_or_a_forwarding_terminator() {
        use lucidos_gateway_tls::Connection;
        let tls = Connection::Tls;
        let plain = Connection::Plain;
        assert_eq!(CookieForm::of(&headers(&[]), &tls), CookieForm::Hardened);
        assert_eq!(CookieForm::of(&headers(&[]), &plain), CookieForm::Plain);
        let fwd = headers(&[("x-forwarded-proto", "https")]);
        assert_eq!(CookieForm::of(&fwd, &plain), CookieForm::Hardened);
        let chain = headers(&[("x-forwarded-proto", "https, http")]);
        assert_eq!(CookieForm::of(&chain, &plain), CookieForm::Hardened);
        let http = headers(&[("x-forwarded-proto", "http")]);
        assert_eq!(CookieForm::of(&http, &plain), CookieForm::Plain);
        let redirecting = Connection::PlainWithHttps {
            authority: "node.example.ts.net:5252".into(),
        };
        assert_eq!(
            CookieForm::of(&headers(&[]), &redirecting),
            CookieForm::Plain
        );
    }

    #[test]
    fn each_form_reads_only_its_own_cookie() {
        let paired = paired_with("cred-abc");
        let plain = headers(&[("cookie", &format!("{OWN_COOKIE}=cred-abc"))]);
        let hardened = headers(&[("cookie", &format!("__Host-{OWN_COOKIE}=cred-abc"))]);
        let decide =
            |h: &HeaderMap, form| authorize_with_match(h, "secret", &paired, OWN_COOKIE, form).0;
        assert!(matches!(
            decide(&hardened, CookieForm::Hardened),
            Authorization::Device { .. }
        ));
        assert!(matches!(
            decide(&plain, CookieForm::Plain),
            Authorization::Device { .. }
        ));
        assert_eq!(
            decide(&plain, CookieForm::Hardened),
            Authorization::Unauthorized
        );
        assert_eq!(
            decide(&hardened, CookieForm::Plain),
            Authorization::Unauthorized
        );
    }

    #[test]
    fn the_hardened_reader_takes_our_own_name_as_ours() {
        let h = headers(&[("cookie", &format!("__Host-{OWN_COOKIE}=v"))]);
        let found = presented_credentials(&h, OWN_COOKIE, CookieForm::Hardened);
        assert_eq!(found.len(), 1);
        assert!(!found[0].is_renamed());
        let audit = presented_credential_summary(&h, OWN_COOKIE, CookieForm::Hardened);
        assert!(audit.own_name);
    }

    #[test]
    fn no_other_file_formats_a_device_cookie() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut dirs = vec![src];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                if name == "auth.rs" || name.ends_with("_tests.rs") || !name.ends_with(".rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                let shipped = text.split("#[cfg(test)]").next().unwrap_or_default();
                if shipped.contains("HttpOnly") {
                    offenders.push(path.display().to_string());
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "a second cookie formatter: {offenders:?}"
        );
    }

    #[test]
    fn an_outbound_credential_stays_out_of_debug_output() {
        let mut headers = HeaderMap::new();
        headers.insert(
            HEADER_LOCAL_TOKEN,
            sensitive_credential("aaaa1111").expect("a plain token builds"),
        );
        headers.insert(
            HEADER_HOOK_SOCKET_TOKEN,
            sensitive_credential("bbbb2222").expect("a plain token builds"),
        );
        let rendered = format!("{headers:?}");
        assert!(!rendered.contains("aaaa1111"), "{rendered}");
        assert!(!rendered.contains("bbbb2222"), "{rendered}");
    }

    #[test]
    fn a_credential_that_will_not_build_yields_no_header() {
        assert!(sensitive_credential("with a\nnewline").is_none());
        assert!(sensitive_credential("\u{7f}").is_none());
        assert!(sensitive_credential("ordinary-token").is_some());
    }

    #[test]
    fn the_paired_store_round_trips_and_a_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("paired-devices.json");
        assert_eq!(
            PairedDevices::load(&path).unwrap(),
            PairedDevices::default()
        );

        let store = paired_with("cred-abc");
        store.save(&path).unwrap();
        assert_eq!(PairedDevices::load(&path).unwrap(), store);

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("cred-abc"), "the store leaked a credential");
    }

    #[test]
    fn a_store_written_before_last_seen_existed_still_pairs_every_device() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("paired-devices.json");
        std::fs::write(
            &path,
            r#"{"devices":[{"id":"device-1","label":"My iPhone",
               "credential_digest":"abc","paired_at":"2026-08-17T00:00:00Z"}]}"#,
        )
        .unwrap();

        let store = PairedDevices::load(&path).unwrap();
        assert_eq!(store.devices.len(), 1);
        assert_eq!(store.devices[0].last_seen_at, None);
        assert_eq!(store.devices[0].label, "My iPhone");
    }

    #[test]
    fn a_last_seen_restamp_is_due_daily_and_not_more_often() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-08-19T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let ago = |h: i64| (now - chrono::Duration::hours(h)).to_rfc3339();

        assert!(last_seen_is_due(None, now));
        assert!(last_seen_is_due(Some(&ago(25)), now));
        assert!(last_seen_is_due(Some(&ago(24)), now));

        assert!(!last_seen_is_due(Some(&ago(23)), now));
        assert!(!last_seen_is_due(Some(&now.to_rfc3339()), now));
    }

    #[test]
    fn an_unreadable_last_seen_restamps_and_a_future_one_does_not() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-08-19T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert!(last_seen_is_due(Some("not a timestamp"), now));
        assert!(last_seen_is_due(Some(""), now));
        assert!(!last_seen_is_due(Some("2027-01-01T00:00:00Z"), now));
    }

    #[test]
    fn the_store_lives_under_the_gateway_s_own_data_dir() {
        let a = paired_devices_path(Path::new("/tmp/gw-a"));
        let b = paired_devices_path(Path::new("/tmp/gw-b"));
        assert_ne!(a, b);
        assert!(a.ends_with("paired-devices.json"));
    }

    #[test]
    fn a_malformed_store_errors_rather_than_forgetting_every_device() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("paired-devices.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(
            PairedDevices::load(&path).is_err(),
            "an empty store here would read as 'nothing was ever paired'"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_paired_store_is_written_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("paired-devices.json");
        paired_with("cred-abc").save(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
