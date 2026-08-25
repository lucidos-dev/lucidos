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

/// The cookie name every gateway wrote before the names were split.
///
/// Still READ, never written. A browser paired before the split holds its
/// credential here, and refusing it would meet that device with the pairing
/// screen. [`authorize`] therefore falls back to it, and the response re-issues
/// the same credential under this gateway's own name.
pub const LEGACY_COOKIE_DEVICE_CREDENTIAL: &str = "lucidos_device";

/// This gateway's cookie name, `lucidos_device_<id>`.
///
/// Per gateway, because a cookie is scoped to the HOST and ignores the port.
/// Two gateways on one hostname therefore share one cookie slot, and one shared
/// name made each pairing evict the other. Measured before this split: pairing
/// the second gateway took the first from 200 to 401.
///
/// The id digests the data dir, which is what scopes the device store too. So
/// the cookie and the store agree on which gateway they belong to. Moving a
/// data dir renames the cookie and asks its devices to pair again, which is
/// what moving the store already does.
///
/// `HttpOnly` is load-bearing rather than hygiene, whatever the name. App
/// iframes are served same-origin with `allow-same-origin`, so a credential
/// JavaScript can read is one an app can ship off-machine.
pub fn device_cookie_name(app_data: &Path) -> String {
    let id = &digest(&app_data.to_string_lossy())[..8];
    format!("{LEGACY_COOKIE_DEVICE_CREDENTIAL}_{id}")
}

/// Which cookie carried the credential on this request.
///
/// The distinction is what drives the one-time migration: a credential that
/// arrived under the legacy name is re-issued under this gateway's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentedCredential<'a> {
    /// This gateway's own cookie.
    Own(&'a str),
    /// The pre-split shared name.
    Legacy(&'a str),
}

impl<'a> PresentedCredential<'a> {
    pub fn value(self) -> &'a str {
        match self {
            PresentedCredential::Own(v) | PresentedCredential::Legacy(v) => v,
        }
    }

    /// Does this need re-issuing under the gateway's own name?
    pub fn is_legacy(self) -> bool {
        matches!(self, PresentedCredential::Legacy(_))
    }
}

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

/// A gateway's paired-device store, `<data dir>/paired-devices.json`.
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

/// The paired-device store for a gateway whose data dir is `app_data`.
///
/// Per gateway, deliberately not machine-global. Two run side by side on one
/// machine: the dev gateway on 5251, the packaged app on 5252. Their data dirs
/// are what separates them. A store keyed on `HOME` handed both one file. Each
/// loaded it once, then rewrote it whole from private memory, so every write
/// deleted the other's devices. A device now pairs to a gateway.
pub fn paired_devices_path(app_data: &Path) -> PathBuf {
    app_data.join("paired-devices.json")
}

/// The pre-isolation store at `~/.lucidos/paired-devices.json`, now a seed.
///
/// Moving the path alone would refuse every device paired before the upgrade.
/// That is the failure `docs/plans/2026-08-19-nobody-is-stranded-by-the-pairing-update.md`
/// exists to prevent. [`load_or_seed`] copies it rather than moving it. The
/// other gateway needs it too, and none can tell whether the others have
/// already read it.
///
/// `None` only when `HOME` is unset.
pub fn legacy_paired_devices_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".lucidos/paired-devices.json"))
}

/// What [`load_or_seed`] found, for the caller's boot log.
pub struct SeededDevices {
    pub devices: PairedDevices,
    /// How many rows came from the legacy store. `None` when it was not read.
    pub seeded_from_legacy: Option<usize>,
}

/// Load this gateway's store, seeding it from `legacy` the first time.
///
/// The seed is written straight back, so the next boot reads this gateway's own
/// file and the legacy one is never consulted again. An absent legacy store is
/// the ordinary first-run case. It yields an empty store rather than an error.
pub fn load_or_seed(path: &Path, legacy: Option<&Path>) -> Result<SeededDevices, crate::BoxError> {
    if path.exists() {
        return Ok(SeededDevices {
            devices: PairedDevices::load(path)?,
            seeded_from_legacy: None,
        });
    }
    let Some(legacy) = legacy.filter(|p| p.exists()) else {
        return Ok(SeededDevices {
            devices: PairedDevices::default(),
            seeded_from_legacy: None,
        });
    };
    let devices = PairedDevices::load(legacy)?;
    devices.save(path)?;
    let count = devices.devices.len();
    Ok(SeededDevices {
        devices,
        seeded_from_legacy: Some(count),
    })
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
/// `name` is this gateway's own ([`device_cookie_name`]), never the legacy one:
/// writing that would put the two gateways back in one slot. `HttpOnly` always,
/// for the app-iframe reason there. `SameSite=Lax` blocks the cross-site POST
/// shape of CSRF while keeping a normal top-level navigation working. `Secure`
/// is conditional, because setting it on a plain-http origin makes the browser
/// drop the cookie and pairing would fail with nothing to see.
pub fn credential_cookie(name: &str, credential: &str, secure: bool) -> String {
    let mut cookie =
        format!("{name}={credential}; Path=/; HttpOnly; SameSite=Lax; Max-Age=31536000");
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

/// The `Set-Cookie` value that clears a device's credential.
///
/// This gateway's name only. Clearing the legacy one would sign the browser out
/// of every OTHER gateway on the host, which is the eviction the split removed.
/// Leaving it costs nothing: the revoked credential resolves to no device here.
pub fn cleared_credential_cookie(name: &str) -> String {
    format!("{name}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
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
pub fn authorize(
    headers: &HeaderMap,
    local_token: &str,
    paired: &PairedDevices,
    cookie_name: &str,
) -> Authorization {
    if presented_local_token(headers).is_some_and(|t| ct_eq(t, local_token)) {
        return Authorization::LocalProcess;
    }
    if let Some(presented) = presented_credential(headers, cookie_name) {
        if let Some(device) = paired.match_credential(presented.value()) {
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

/// The device credential a request presented, and which name carried it.
///
/// This gateway's own name wins. The legacy name is the fallback, so a browser
/// paired before the split still gets in. Both are read here rather than in
/// [`authorize`], so one rule decides what counts as presented.
///
/// Exposed so a caller re-issuing the cookie can read the value back off the
/// request. The alternative was carrying it in [`Authorization::Device`]. That
/// would put a bearer secret in the value naming a device, and so in every
/// place that matches on one.
pub fn presented_credential<'a>(
    headers: &'a HeaderMap,
    cookie_name: &str,
) -> Option<PresentedCredential<'a>> {
    if let Some(v) = cookie_value(headers, cookie_name) {
        return Some(PresentedCredential::Own(v));
    }
    cookie_value(headers, LEGACY_COOKIE_DEVICE_CREDENTIAL).map(PresentedCredential::Legacy)
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

    /// One gateway's cookie name, so a test says which gateway it means. The
    /// tests below present the LEGACY name on purpose, which is the fallback.
    const OWN_COOKIE: &str = "lucidos_device_deadbeef";

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
        // `Some("   ")` must not read as "present, therefore fine", and it must
        // never compare equal to an empty configured token either.
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
        // ADR 0050: the device id names who to credit, never what they may do.
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

        // A name that merely ENDS WITH ours must not satisfy the lookup.
        let h = headers(&[("cookie", "xlucidos_device=cred-abc")]);
        assert_eq!(
            authorize(&h, "secret", &paired, OWN_COOKIE),
            Authorization::Unauthorized
        );
    }

    // ── The cookie name is per gateway ──────────────────────────────────────

    #[test]
    fn two_data_dirs_get_two_cookie_names() {
        // A cookie is scoped to the HOST and ignores the port, so two gateways
        // on one hostname share a slot. One name made each pairing evict the
        // other. Measured: the first gateway went from 200 to 401.
        let a = device_cookie_name(Path::new("/tmp/gw-a"));
        let b = device_cookie_name(Path::new("/tmp/gw-b"));
        assert_ne!(a, b);
        assert!(a.starts_with("lucidos_device_"), "{a}");
        // Stable, or every restart would sign every device out.
        assert_eq!(a, device_cookie_name(Path::new("/tmp/gw-a")));
        // And never the legacy name, which is the contested slot itself.
        assert_ne!(a, LEGACY_COOKIE_DEVICE_CREDENTIAL);
    }

    #[test]
    fn this_gateways_own_cookie_wins_over_the_legacy_one() {
        // A browser mid-migration holds both. The gateway's own is the fresher
        // of the two, and the legacy slot may belong to another gateway by now.
        let own = device_cookie_name(Path::new("/tmp/gw-a"));
        let h = headers(&[(
            "cookie",
            &format!("lucidos_device=legacy-value; {own}=own-value"),
        )]);
        assert_eq!(
            presented_credential(&h, &own),
            Some(PresentedCredential::Own("own-value"))
        );
    }

    #[test]
    fn a_legacy_cookie_is_read_when_this_gateway_has_none_yet() {
        // Nobody paired before the split is locked out. The caller re-issues
        // under the gateway's own name once it sees this.
        let own = device_cookie_name(Path::new("/tmp/gw-a"));
        let h = headers(&[("cookie", "lucidos_device=legacy-value")]);
        let presented = presented_credential(&h, &own).expect("the legacy name is a fallback");
        assert_eq!(presented, PresentedCredential::Legacy("legacy-value"));
        assert!(presented.is_legacy());
        assert_eq!(presented.value(), "legacy-value");
    }

    #[test]
    fn another_gateways_cookie_is_not_read_as_ours() {
        // The whole point of the split: B's cookie must not authorize at A,
        // and must not be mistaken for the legacy fallback either.
        let a = device_cookie_name(Path::new("/tmp/gw-a"));
        let b = device_cookie_name(Path::new("/tmp/gw-b"));
        let h = headers(&[("cookie", &format!("{b}=b-value"))]);
        assert_eq!(presented_credential(&h, &a), None);
    }

    // The local token's own behaviour (hex width, 0600 creation, mode repair,
    // constant-time compare) is tested in `lucidos-local-token`, which owns it.

    #[test]
    fn the_credential_cookie_is_always_httponly_and_lax() {
        // An app iframe is same-origin, so a readable credential is one an app
        // can exfiltrate. This attribute is the whole defence.
        let cookie = credential_cookie(OWN_COOKIE, "abc", false);
        // Written under this gateway's name, never the shared legacy one.
        assert!(
            cookie.starts_with(&format!("{OWN_COOKIE}=abc;")),
            "{cookie}"
        );
        assert!(cookie.contains("HttpOnly"), "{cookie}");
        assert!(cookie.contains("SameSite=Lax"), "{cookie}");
        assert!(cookie.contains("Path=/"), "{cookie}");
        assert!(!cookie.contains("Secure"), "plain http must not set Secure");
        assert!(credential_cookie(OWN_COOKIE, "abc", true).contains("; Secure"));
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

    // ── The store is per gateway, and the old path seeds it ─────────────────

    #[test]
    fn the_store_lives_under_the_gateway_s_own_data_dir() {
        // Two gateways run on one machine, so this must not resolve to one
        // shared path. `HOME` is what it used to read, and does not appear.
        let a = paired_devices_path(Path::new("/tmp/gw-a"));
        let b = paired_devices_path(Path::new("/tmp/gw-b"));
        assert_ne!(a, b);
        assert!(a.ends_with("paired-devices.json"));
    }

    #[test]
    fn the_legacy_store_seeds_a_gateway_that_has_none_yet() {
        // The upgrade path. Moving the store without this refuses every device
        // paired before it, on both gateways at once.
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("legacy.json");
        let mine = dir.path().join("gw/paired-devices.json");
        paired_with("cred-abc").save(&legacy).unwrap();

        let seeded = load_or_seed(&mine, Some(&legacy)).unwrap();
        assert_eq!(seeded.seeded_from_legacy, Some(1));
        assert_eq!(seeded.devices.devices[0].label, "My iPhone");
        // Written straight back, so the next boot reads its own file.
        assert_eq!(PairedDevices::load(&mine).unwrap(), seeded.devices);
    }

    #[test]
    fn the_legacy_store_is_read_and_never_written() {
        // It is copied rather than moved: the OTHER gateway still has to seed
        // from it, and no gateway can tell whether it already has.
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("legacy.json");
        paired_with("cred-abc").save(&legacy).unwrap();
        let before = std::fs::read_to_string(&legacy).unwrap();

        let mine = dir.path().join("gw/paired-devices.json");
        load_or_seed(&mine, Some(&legacy)).unwrap();
        let mut store = PairedDevices::load(&mine).unwrap();
        store.devices.clear();
        store.save(&mine).unwrap();

        assert_eq!(std::fs::read_to_string(&legacy).unwrap(), before);
    }

    #[test]
    fn a_gateway_with_its_own_store_ignores_the_legacy_one() {
        // Seeding once is the whole contract. Re-seeding would put back every
        // device this gateway had revoked since.
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("legacy.json");
        paired_with("cred-abc").save(&legacy).unwrap();
        let mine = dir.path().join("mine.json");
        PairedDevices::default().save(&mine).unwrap();

        let seeded = load_or_seed(&mine, Some(&legacy)).unwrap();
        assert_eq!(seeded.seeded_from_legacy, None);
        assert!(seeded.devices.devices.is_empty());
    }

    #[test]
    fn a_first_run_with_no_legacy_store_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mine = dir.path().join("mine.json");
        let missing = dir.path().join("nothing-here.json");

        for legacy in [None, Some(missing.as_path())] {
            let seeded = load_or_seed(&mine, legacy).unwrap();
            assert_eq!(seeded.seeded_from_legacy, None);
            assert!(seeded.devices.devices.is_empty());
            assert!(!mine.exists(), "a fresh install creates no store");
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_seeded_copy_is_owner_only_too() {
        // The seed is a write like any other, and it carries credential
        // digests. A world-readable copy of the store is the same leak.
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("legacy.json");
        paired_with("cred-abc").save(&legacy).unwrap();
        let mine = dir.path().join("gw/paired-devices.json");
        load_or_seed(&mine, Some(&legacy)).unwrap();
        let mode = std::fs::metadata(&mine).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
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
