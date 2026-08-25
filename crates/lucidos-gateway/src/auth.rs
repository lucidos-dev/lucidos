//! Inbound authentication for the gateway.
//!
//! Full design and its rejected alternatives:
//! `docs/plans/2026-08-17-clients-pair-to-the-gateway-and-webhooks-get-their-own-socket.md`.
//!
//! # A loopback peer address authorizes nothing
//!
//! "Trust `127.0.0.1`" fails open here. The shipped remote-access route is
//! `tailscale serve --bg --https=443 http://127.0.0.1:5252`, and Serve proxies
//! from *this* machine. A phone's request therefore arrives with a loopback
//! peer address, so trusting loopback would trust the whole tailnet.
//!
//! [`authorize`] takes no peer address. The signature holds the invariant,
//! rather than everyone remembering it. Proving locality is instead reading a
//! mode 0600 file, which `lucidos-local-token` owns for every crate that needs
//! it. `x-lucidos-device-id` is not read here: ADR 0050 makes it a display
//! hint, never an authorization input.
//!
//! The gateway does WRITE that header. [`crate::auth_api::enforce`] stamps the
//! authenticated device on the request, and the proxy re-injects it. So the
//! engine keys its per-workspace device state on the device that paired. That
//! is an output of this decision, never an input to it.

use axum::http::HeaderMap;
use lucidos_local_token as local_token;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub use local_token::{ct_eq, path as local_token_path, HEADER_LOCAL_TOKEN};

/// Cookie carrying a paired device's credential.
///
/// `HttpOnly` is load-bearing rather than hygiene. App iframes are served
/// same-origin with `allow-same-origin`, so a credential JavaScript can read is
/// one an app can ship off-machine. An app can still *use* the cookie, which is
/// the authority it already has today, but it cannot steal it.
pub const COOKIE_DEVICE_CREDENTIAL: &str = "lucidos_device";

/// How long a pairing code stays redeemable. Long enough to walk to another
/// device, short enough that a code read over a shoulder goes stale.
const PAIRING_CODE_TTL: Duration = Duration::from_secs(300);

/// Digits in a pairing code. Short enough to type on a phone. Its safety comes
/// from the TTL and single use, not from the digits alone.
const PAIRING_CODE_DIGITS: u32 = 8;

/// What a request proved about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Authorization {
    /// A process on this machine, proved by the mode 0600 token file.
    LocalProcess,
    /// A paired device, proved by its credential cookie.
    Device { id: String, label: String },
    /// Nothing was proved. The caller gets no access.
    Unauthorized,
}

/// The device id [`crate::auth_api::enforce`] resolved, stamped onto the
/// request for the proxy to forward.
///
/// A request extension rather than a header, because a client can send a header
/// and cannot send an extension. The proxy strips every inbound
/// `x-lucidos-device-id` and re-injects from this, so an absent extension
/// forwards no id at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedDevice(pub String);

/// One paired device, as persisted.
///
/// `credential_digest` is a SHA-256 of the credential, never the credential.
/// The local token in the sibling file stays plaintext because a caller sends
/// it back for comparison. A device credential is different: it is a bearer
/// cookie held by a remote device. Storing it in the clear would turn one
/// leaked file into durable remote access, so it is digested.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairedDevice {
    pub id: String,
    pub label: String,
    pub credential_digest: String,
    pub paired_at: String,
    /// When this device last reached us, to the nearest day. `None` for a
    /// device paired before the field existed, which fills in on its next
    /// request rather than being backfilled.
    ///
    /// A liveness hint for the devices list, never an auth input: `paired_at`
    /// alone makes every row look alike, so nobody can tell a phone in daily
    /// use from a laptop they sold. Stamped at most daily, so it is not an
    /// access log and cannot become one.
    #[serde(default)]
    pub last_seen_at: Option<String>,
}

/// How stale a device's `last_seen_at` gets before the next request restamps
/// it. A day, because the list reads in days and a finer beat would only buy
/// writes.
pub const LAST_SEEN_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Pure: is this stored `last_seen_at` due a restamp at `now`?
///
/// `None` is due, which is what fills in a device paired before the field
/// existed. An unparseable value is due too: a row nothing can read is one no
/// throttle should protect. A value in the future is NOT due, so a clock that
/// jumped back cannot make every request write.
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

/// The machine-global paired-device store, `~/.lucidos/paired-devices.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairedDevices {
    #[serde(default)]
    pub devices: Vec<PairedDevice>,
}

impl PairedDevices {
    /// Load the store. A missing file is an empty store, which is the first-run
    /// state. A malformed one is an error rather than an empty store: silently
    /// forgetting every paired device would lock the user out of their own
    /// machine and read as "nothing was ever paired".
    pub fn load(path: &Path) -> Result<Self, crate::BoxError> {
        match std::fs::read_to_string(path) {
            Ok(raw) if raw.trim().is_empty() => Ok(Self::default()),
            Ok(raw) => Ok(serde_json::from_str(&raw)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(Box::new(e)),
        }
    }

    /// Write the store atomically, owner-only.
    pub fn save(&self, path: &Path) -> Result<(), crate::BoxError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        write_owner_only(&tmp, &serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// The device whose credential digest matches, if any.
    fn match_credential(&self, credential: &str) -> Option<&PairedDevice> {
        let digest = digest(credential);
        self.devices
            .iter()
            .find(|d| ct_eq(&d.credential_digest, &digest))
    }
}

/// `~/.lucidos/paired-devices.json`. `None` only when `HOME` is unset.
pub fn paired_devices_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".lucidos/paired-devices.json"))
}

/// Lowercase-hex SHA-256 of `value`.
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

/// Pairing codes that have been minted and not yet redeemed.
///
/// In memory only, and deliberately so. A code lives five minutes, so surviving
/// a restart buys nothing. Persisting it would put a credential-minting secret
/// on disk for no gain.
#[derive(Debug, Default)]
pub struct PendingPairings {
    codes: HashMap<String, PendingPairing>,
}

/// One outstanding code, with the label whoever minted it suggested.
#[derive(Debug)]
struct PendingPairing {
    minted: Instant,
    /// What `lucidos pair --label` asked this device to be called. Carried
    /// through redemption so the flag does what it says, rather than only
    /// printing a name into the terminal.
    label: Option<String>,
}

impl PendingPairings {
    /// Mint a code and remember it until it is redeemed or expires.
    pub fn mint(&mut self, label: Option<String>) -> std::io::Result<String> {
        self.drop_expired();
        let code = mint_numeric_code()?;
        self.codes.insert(
            code.clone(),
            PendingPairing {
                minted: Instant::now(),
                label,
            },
        );
        Ok(code)
    }

    /// Redeem a code, consuming it.
    ///
    /// `None` means the code was wrong or expired. `Some(label)` carries the
    /// name suggested when it was minted, which may itself be `None`. A code
    /// works once: a second redemption of an observed code must not enrol a
    /// second device.
    pub fn redeem(&mut self, presented: &str) -> Option<Option<String>> {
        self.drop_expired();
        // Found by constant-time compare rather than `HashMap::remove`, so a
        // wrong code cannot be narrowed down by how long the lookup took.
        let key = self
            .codes
            .keys()
            .find(|k| ct_eq(k, presented.trim()))
            .cloned()?;
        self.codes.remove(&key).map(|pending| pending.label)
    }

    fn drop_expired(&mut self) {
        self.codes
            .retain(|_, pending| pending.minted.elapsed() < PAIRING_CODE_TTL);
    }
}

/// A fresh device credential: 32 bytes of entropy, lowercase hex.
pub fn mint_credential() -> std::io::Result<String> {
    local_token::mint_hex(32)
}

/// How long a minted pairing code stays redeemable, in seconds. Reported to the
/// client so the pairing screen can say it rather than guess.
pub fn pairing_code_ttl_secs() -> u64 {
    PAIRING_CODE_TTL.as_secs()
}

/// Render the `Set-Cookie` value that hands a device its credential.
///
/// `HttpOnly` always, for the app-iframe reason on [`COOKIE_DEVICE_CREDENTIAL`].
/// `SameSite=Lax` blocks the cross-site POST shape of CSRF while keeping a
/// normal top-level navigation working. `Secure` is conditional, because
/// setting it on a plain-http origin makes the browser drop the cookie and
/// pairing would fail with nothing to see.
pub fn credential_cookie(credential: &str, secure: bool) -> String {
    let mut cookie = format!(
        "{COOKIE_DEVICE_CREDENTIAL}={credential}; Path=/; HttpOnly; SameSite=Lax; Max-Age=31536000"
    );
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

/// The `Set-Cookie` value that clears a device's credential.
pub fn cleared_credential_cookie() -> String {
    format!("{COOKIE_DEVICE_CREDENTIAL}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
}

/// Did this request reach us over TLS?
///
/// `served_tls` is what our own socket terminated. The forwarded header covers
/// the shipped case where something else terminated TLS and proxied to us,
/// which is what `tailscale serve` does. This decides only whether to mark a
/// cookie `Secure`, never whether to authorize, so a spoofed header buys a
/// caller nothing.
pub fn request_is_secure(headers: &HeaderMap, served_tls: bool) -> bool {
    if served_tls {
        return true;
    }
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').next().is_some_and(|p| p.trim() == "https"))
}

/// `raw` if it is a pairing code exactly as minted, trimmed.
///
/// The reader for what [`mint_numeric_code`] writes, so one grammar governs
/// both. Its strictness is load-bearing. A caller-supplied code is echoed into
/// the PWA manifest's `start_url`, and that is a JSON document the browser
/// reads. Decimal digits cannot escape any context they land in.
pub fn valid_pairing_code(raw: &str) -> Option<&str> {
    let code = raw.trim();
    let well_formed =
        code.len() == PAIRING_CODE_DIGITS as usize && code.bytes().all(|b| b.is_ascii_digit());
    well_formed.then_some(code)
}

/// A zero-padded decimal pairing code, drawn from the same entropy source as
/// every other secret here.
fn mint_numeric_code() -> std::io::Result<String> {
    use std::io::Read as _;
    let mut buf = [0u8; 8];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut buf)?;
    let modulus = 10u64.pow(PAIRING_CODE_DIGITS);
    let value = u64::from_be_bytes(buf) % modulus;
    Ok(format!(
        "{value:0width$}",
        width = PAIRING_CODE_DIGITS as usize
    ))
}

/// Read the local token, minting it on first use. The gateway is the only
/// process that mints; everyone else calls `lucidos_local_token::read`.
pub fn ensure_local_token() -> std::io::Result<String> {
    local_token::ensure()
}

/// A fresh device id. Not a secret: it names a row in the paired-device store
/// and appears in the devices list, so 8 bytes is plenty to avoid collisions.
pub fn mint_device_id() -> std::io::Result<String> {
    local_token::mint_hex(8)
}

/// Write `contents` to `path` with mode 0600, never create-then-chmod.
fn write_owner_only(path: &Path, contents: &str) -> std::io::Result<()> {
    local_token::write_owner_only(path, contents)
}

/// Decide what an inbound request proved.
///
/// Takes no peer address on purpose: see the module docs. A caller wanting to
/// "just allow loopback" has nothing here to do it with.
pub fn authorize(headers: &HeaderMap, local_token: &str, paired: &PairedDevices) -> Authorization {
    if presented_local_token(headers).is_some_and(|t| ct_eq(t, local_token)) {
        return Authorization::LocalProcess;
    }
    if let Some(credential) = cookie_value(headers, COOKIE_DEVICE_CREDENTIAL) {
        if let Some(device) = paired.match_credential(credential) {
            return Authorization::Device {
                id: device.id.clone(),
                label: device.label.clone(),
            };
        }
    }
    Authorization::Unauthorized
}

/// The local token a request presented, trimmed and non-empty.
fn presented_local_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(HEADER_LOCAL_TOKEN)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// The device credential a request presented, if any.
///
/// Exposed so a caller re-issuing the cookie can read the value back off the
/// request. The alternative was carrying it in [`Authorization::Device`]. That
/// would put a bearer secret in the value naming a device, and so in every
/// place that matches on one.
pub fn presented_credential(headers: &HeaderMap) -> Option<&str> {
    cookie_value(headers, COOKIE_DEVICE_CREDENTIAL)
}

/// One cookie's value out of the `Cookie` header.
///
/// Hand-parsed rather than pulling in a cookie crate: the gateway is the only
/// network-facing process and its dependency list is kept short. The format is
/// `name=value` pairs joined by `; `, and a name is matched exactly so
/// `xlucidos_device` never satisfies a lookup for `lucidos_device`.
fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())?
        .split(';')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| k.trim() == name)
        .map(|(_, v)| v.trim())
        .filter(|v| !v.is_empty())
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
            authorize(&headers(&[]), "secret", &none),
            Authorization::Unauthorized
        );
    }

    #[test]
    fn the_local_token_proves_a_local_process() {
        let none = PairedDevices::default();
        let h = headers(&[(HEADER_LOCAL_TOKEN, "secret")]);
        assert_eq!(authorize(&h, "secret", &none), Authorization::LocalProcess);
    }

    #[test]
    fn a_wrong_local_token_is_unauthorized() {
        let none = PairedDevices::default();
        let h = headers(&[(HEADER_LOCAL_TOKEN, "nope")]);
        assert_eq!(authorize(&h, "secret", &none), Authorization::Unauthorized);
    }

    #[test]
    fn a_blank_local_token_is_unauthorized() {
        // `Some("   ")` must not read as "present, therefore fine", and it must
        // never compare equal to an empty configured token either.
        let none = PairedDevices::default();
        let h = headers(&[(HEADER_LOCAL_TOKEN, "   ")]);
        assert_eq!(authorize(&h, "secret", &none), Authorization::Unauthorized);
        assert_eq!(authorize(&h, "", &none), Authorization::Unauthorized);
    }

    #[test]
    fn a_device_id_header_authorizes_nothing() {
        // ADR 0050: the device id names who to credit, never what they may do.
        let none = PairedDevices::default();
        let h = headers(&[("x-lucidos-device-id", "some-real-device-id")]);
        assert_eq!(authorize(&h, "secret", &none), Authorization::Unauthorized);
    }

    #[test]
    fn a_paired_credential_cookie_names_its_device() {
        let paired = paired_with("cred-abc");
        let h = headers(&[("cookie", "lucidos_device=cred-abc")]);
        assert_eq!(
            authorize(&h, "secret", &paired),
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
            authorize(&h, "secret", &paired),
            Authorization::Unauthorized
        );
    }

    #[test]
    fn the_cookie_is_found_among_siblings_and_matched_by_exact_name() {
        let paired = paired_with("cred-abc");
        let h = headers(&[("cookie", "theme=dark; lucidos_device=cred-abc; other=1")]);
        assert!(matches!(
            authorize(&h, "secret", &paired),
            Authorization::Device { .. }
        ));

        // A name that merely ENDS WITH ours must not satisfy the lookup.
        let h = headers(&[("cookie", "xlucidos_device=cred-abc")]);
        assert_eq!(
            authorize(&h, "secret", &paired),
            Authorization::Unauthorized
        );
    }

    // The local token's own behaviour (hex width, 0600 creation, mode repair,
    // constant-time compare) is tested in `lucidos-local-token`, which owns it.

    #[test]
    fn the_credential_cookie_is_always_httponly_and_lax() {
        // An app iframe is same-origin, so a readable credential is one an app
        // can exfiltrate. This attribute is the whole defence.
        let cookie = credential_cookie("abc", false);
        assert!(cookie.contains("HttpOnly"), "{cookie}");
        assert!(cookie.contains("SameSite=Lax"), "{cookie}");
        assert!(cookie.contains("Path=/"), "{cookie}");
        assert!(!cookie.contains("Secure"), "plain http must not set Secure");
        assert!(credential_cookie("abc", true).contains("; Secure"));
    }

    #[test]
    fn secure_follows_our_own_tls_or_a_forwarding_terminator() {
        assert!(request_is_secure(&headers(&[]), true));
        assert!(!request_is_secure(&headers(&[]), false));
        let fwd = headers(&[("x-forwarded-proto", "https")]);
        assert!(request_is_secure(&fwd, false));
        // A proxy chain appends, so only the first hop is ours to read.
        let chain = headers(&[("x-forwarded-proto", "https, http")]);
        assert!(request_is_secure(&chain, false));
        let plain = headers(&[("x-forwarded-proto", "http")]);
        assert!(!request_is_secure(&plain, false));
    }

    #[test]
    fn a_pairing_code_redeems_once_and_then_never_again() {
        let mut pending = PendingPairings::default();
        let code = pending.mint(None).unwrap();
        assert_eq!(code.len(), PAIRING_CODE_DIGITS as usize);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
        assert!(
            pending.redeem(&code).is_some(),
            "the first redemption must work"
        );
        assert!(
            pending.redeem(&code).is_none(),
            "an observed code must not enrol a second device"
        );
    }

    #[test]
    fn an_unminted_code_never_redeems() {
        let mut pending = PendingPairings::default();
        pending.mint(None).unwrap();
        assert!(pending.redeem("00000000").is_none());
        assert!(pending.redeem("").is_none());
    }

    #[test]
    fn a_code_reader_accepts_exactly_what_the_minter_writes() {
        let mut pending = PendingPairings::default();
        let minted = pending.mint(None).unwrap();
        assert_eq!(valid_pairing_code(&minted), Some(minted.as_str()));
        assert_eq!(valid_pairing_code("01234567"), Some("01234567"));
        assert_eq!(valid_pairing_code("  01234567  "), Some("01234567"));
    }

    #[test]
    fn a_code_reader_refuses_anything_that_could_escape_a_json_string() {
        // This value is echoed into the manifest's `start_url`, so the grammar
        // is the whole defence. Each of these is a caller's to send.
        for raw in [
            "",
            "   ",
            "0123456",
            "012345678",
            "0123456a",
            "0123 567",
            "0123456\"",
            "01234567\"}",
            "0123456\n",
            "../../etc",
            "01234567&x=1",
            "%30%31%32%33%34%35%36%37",
        ] {
            assert_eq!(valid_pairing_code(raw), None, "accepted {raw:?}");
        }
    }

    #[test]
    fn the_label_given_at_mint_survives_redemption() {
        // `lucidos pair --label` printed a name and attached it to nothing, so
        // the device landed under the default. The label rides the code now.
        let mut pending = PendingPairings::default();
        let code = pending.mint(Some("My iPhone".into())).unwrap();
        assert_eq!(pending.redeem(&code), Some(Some("My iPhone".into())));
    }

    #[test]
    fn a_code_minted_without_a_label_redeems_to_no_label() {
        let mut pending = PendingPairings::default();
        let code = pending.mint(None).unwrap();
        assert_eq!(pending.redeem(&code), Some(None));
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

        // The credential itself must never be written.
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("cred-abc"), "the store leaked a credential");
    }

    #[test]
    fn a_store_written_before_last_seen_existed_still_pairs_every_device() {
        // The upgrade path. This file is machine-global user state, so a device
        // dropped here is a person locked out of their own machine.
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

        // Never stamped, so the next request fills it in.
        assert!(last_seen_is_due(None, now));
        assert!(last_seen_is_due(Some(&ago(25)), now));
        assert!(last_seen_is_due(Some(&ago(24)), now));

        // Inside the window nothing is written, which is the whole throttle.
        assert!(!last_seen_is_due(Some(&ago(23)), now));
        assert!(!last_seen_is_due(Some(&now.to_rfc3339()), now));
    }

    #[test]
    fn an_unreadable_last_seen_restamps_and_a_future_one_does_not() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-08-19T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        // A value nothing can read is one no throttle should protect.
        assert!(last_seen_is_due(Some("not a timestamp"), now));
        assert!(last_seen_is_due(Some(""), now));
        // A clock that jumped back must not make every request write.
        assert!(!last_seen_is_due(Some("2027-01-01T00:00:00Z"), now));
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
