//! Pairing secrets, and the channel a redemption arrived on.
//!
//! Entropy is the defence, never a throttle. A flood can spend a throttle, and
//! a spent throttle stands between the user and a correct secret. So:
//!
//! - Every pending pairing holds a **long secret**: 130 random bits, pasted or
//!   carried by a QR or link, never typed. It is accepted on every channel and
//!   is never throttled.
//! - A device pairing also holds an 8-digit **short code** for a person who has
//!   to type. It is accepted only on a proved-local channel, with a per-source
//!   budget behind it. A public caller is refused before any compare.
//! - With nothing pending, a redemption is refused at once and counts nothing.
//!
//! Design and its rejected alternatives: ADR 0615.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant};

use axum::http::HeaderMap;

use crate::auth::ct_eq;
use lucidos_gateway_constants::{
    DEVICE_PAIRING_SECRET_PREFIX, GATEWAY_PAIRING_SECRET_PREFIX, PAIRING_CODE_DIGITS,
    PAIRING_SECRET_CHARS,
};

/// How long a pending pairing stays redeemable. Long enough to walk to another
/// device, short enough that a code read over a shoulder goes stale.
const PAIRING_TTL: Duration = Duration::from_secs(300);

/// Lowercase RFC 4648 base32. Safe unescaped in a URL, JSON and HTML.
const BASE32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// Wrong short codes one local source may send in a window. Forgives the
/// typos a real person makes, and leaves 8 digits hopeless to guess.
const SHORT_CODE_FAILURES_PER_SOURCE: u32 = 10;

/// Wrong short codes from all local sources together in a window. Sized well
/// above one source's cap, so a single source always meets its own cap first.
const SHORT_CODE_FAILURES_GLOBAL: u32 = 50;

/// How long a failure budget takes to refill.
const FAILURE_WINDOW: Duration = Duration::from_secs(60);

/// Headers a tunnel, proxy or access gateway adds. Any one marks a request
/// public. A local browser never sends them, so they can only make a request
/// stricter.
pub(crate) const FORWARDING_HEADERS: [&str; 10] = [
    "cf-connecting-ip",
    "cf-ray",
    "cf-visitor",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-proto",
    "x-real-ip",
    "forwarded",
    "via",
    "tailscale-funnel-request",
];

/// Which grant a pending pairing mints. The two never cross: each has its own
/// [`PendingPairings`] and its own secret prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingKind {
    Device,
    Gateway,
}

impl PairingKind {
    /// The prefix that names a long secret's kind to whoever holds it.
    pub fn secret_prefix(self) -> &'static str {
        match self {
            PairingKind::Device => DEVICE_PAIRING_SECRET_PREFIX,
            PairingKind::Gateway => GATEWAY_PAIRING_SECRET_PREFIX,
        }
    }

    /// Only a device pairing has a short code. A gateway pairing secret is
    /// always pasted, so it never needs one.
    fn has_short_code(self) -> bool {
        matches!(self, PairingKind::Device)
    }
}

/// The socket a request arrived on, stamped by the acceptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Peer {
    pub remote: SocketAddr,
    pub local: SocketAddr,
    /// The listener a tunnel or reverse proxy points at. Every request on it
    /// is public, whatever its headers say.
    pub public_listener: bool,
}

impl Peer {
    /// Did the connection start on this machine? A loopback peer, or one whose
    /// address is the local address it reached.
    pub fn is_same_machine(&self) -> bool {
        let remote = self.remote.ip().to_canonical();
        remote.is_loopback() || remote == self.local.ip().to_canonical()
    }
}

/// Does `headers` carry one of the [`FORWARDING_HEADERS`]?
pub(crate) fn is_forwarded(headers: &HeaderMap) -> bool {
    FORWARDING_HEADERS.iter().any(|h| headers.contains_key(*h))
}

/// Where a redemption came from, as far as the gateway could prove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingChannel {
    /// A direct caller on the LAN or the tailnet. `source` keys
    /// the short-code budget.
    Local { source: IpAddr },
    /// Anything not proved local. The default.
    Public,
}

/// Classify one request. Public unless every local condition holds.
///
/// `is_tailnet_address` answers whether an address of THIS machine is one
/// Tailscale reported.
///
/// A caller on this machine is always public: a loopback peer, or one whose
/// address is the local address it reached. cloudflared, `tailscale serve`,
/// `ssh -L` and a raw TCP forward all connect from this machine, and a raw
/// forward passes whatever `Host` its caller sends. A browser here has the link.
pub fn classify(
    peer: Option<Peer>,
    headers: &HeaderMap,
    is_tailnet_address: impl Fn(IpAddr) -> bool,
) -> PairingChannel {
    let Some(peer) = peer else {
        return PairingChannel::Public;
    };
    if peer.public_listener || is_forwarded(headers) {
        return PairingChannel::Public;
    }
    let remote = peer.remote.ip().to_canonical();
    let local = peer.local.ip().to_canonical();
    let source = if peer.is_same_machine() {
        None
    } else if is_tailnet_address(local) {
        // The control server assigns tailnet addresses, so each is its own source.
        Some(remote)
    } else {
        is_lan_address(remote).then(|| lan_source(remote))
    };
    match source {
        Some(source) => PairingChannel::Local { source },
        None => PairingChannel::Public,
    }
}

/// The source a LAN peer's budget is kept under. An IPv6 host picks any
/// address in its /64, so a key per address would hand it billions of budgets.
fn lan_source(remote: IpAddr) -> IpAddr {
    match remote {
        IpAddr::V6(v6) => {
            let s = v6.segments();
            IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
        }
        v4 => v4,
    }
}

/// A private-range address on a direct socket: RFC 1918, link-local, or an
/// IPv6 unique-local address. `100.64.0.0/10` is not here: other VPNs and ISPs
/// share it, so a tailnet peer is recognised by the local address instead.
fn is_lan_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    }
}

/// `raw` if it is a short code exactly as minted, trimmed.
///
/// Its strictness is load-bearing. A caller-supplied code is echoed into the
/// PWA manifest's `start_url`, and decimal digits cannot escape that context.
pub fn valid_pairing_code(raw: &str) -> Option<&str> {
    let code = raw.trim();
    let well_formed =
        code.len() == PAIRING_CODE_DIGITS as usize && code.bytes().all(|b| b.is_ascii_digit());
    well_formed.then_some(code)
}

/// `raw` if it is a long secret of `kind` exactly as minted, trimmed. Same
/// strictness as [`valid_pairing_code`]: base32 cannot escape a URL or JSON.
pub fn valid_pairing_secret(raw: &str, kind: PairingKind) -> Option<&str> {
    let secret = raw.trim();
    let body = secret.strip_prefix(kind.secret_prefix())?;
    let well_formed =
        body.len() == PAIRING_SECRET_CHARS && body.bytes().all(|b| BASE32.contains(&b));
    well_formed.then_some(secret)
}

/// What a device pairing link may carry: a device secret, or the short code an
/// older gateway put in its QR.
pub fn valid_device_pairing_token(raw: &str) -> Option<&str> {
    valid_pairing_secret(raw, PairingKind::Device).or_else(|| valid_pairing_code(raw))
}

/// One minted pairing: the long secret, plus the short code a device pairing
/// offers a person who has to type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintedPairing {
    pub secret: String,
    pub code: Option<String>,
}

/// What a pairing was minted for: the name the device takes and, for a
/// device pairing, the operator it joins. `None` operator joins the first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PairingIntent {
    pub label: Option<String>,
    pub operator_id: Option<String>,
}

#[cfg(test)]
impl PairingIntent {
    pub fn labelled(label: &str) -> Self {
        Self {
            label: Some(label.to_string()),
            operator_id: None,
        }
    }
}

/// What one redemption attempt produced.
#[derive(Debug, PartialEq, Eq)]
pub enum Redemption {
    /// Good, carrying what it was minted for.
    Redeemed(PairingIntent),
    /// Wrong, expired, or nothing pending.
    Rejected,
    /// A short code from a public channel, refused before any compare.
    LocalOnly,
    /// This source sent too many wrong short codes lately, so this one was not
    /// compared. Only the local short-code path can answer this.
    Throttled,
}

/// One outstanding pairing.
#[derive(Debug)]
struct PendingPairing {
    minted: Instant,
    secret: String,
    code: Option<String>,
    intent: PairingIntent,
}

/// Pairings minted and not yet redeemed.
///
/// In memory only: a pairing lives five minutes, so surviving a restart buys
/// nothing, and persisting one would put a credential-minting secret on disk.
#[derive(Debug)]
pub struct PendingPairings {
    kind: PairingKind,
    pending: Vec<PendingPairing>,
    throttle: ShortCodeThrottle,
}

impl PendingPairings {
    pub fn new(kind: PairingKind) -> Self {
        PendingPairings {
            kind,
            pending: Vec::new(),
            throttle: ShortCodeThrottle::default(),
        }
    }

    /// Mint a pairing and remember it until it is redeemed or expires.
    pub fn mint(&mut self, intent: PairingIntent) -> std::io::Result<MintedPairing> {
        let now = Instant::now();
        self.drop_expired(now);
        let minted = MintedPairing {
            secret: mint_secret(self.kind)?,
            code: if self.kind.has_short_code() {
                Some(mint_short_code()?)
            } else {
                None
            },
        };
        self.pending.push(PendingPairing {
            minted: now,
            secret: minted.secret.clone(),
            code: minted.code.clone(),
            intent,
        });
        Ok(minted)
    }

    /// Redeem a secret or short code, consuming the whole pairing.
    pub fn redeem(&mut self, presented: &str, channel: PairingChannel) -> Redemption {
        self.redeem_at(presented, channel, Instant::now())
    }

    fn redeem_at(&mut self, presented: &str, channel: PairingChannel, now: Instant) -> Redemption {
        self.drop_expired(now);
        if self.pending.is_empty() {
            return Redemption::Rejected;
        }
        if let Some(secret) = valid_pairing_secret(presented, self.kind) {
            return match self.take(|p| ct_eq(&p.secret, secret)) {
                Some(intent) => Redemption::Redeemed(intent),
                None => Redemption::Rejected,
            };
        }
        let Some(code) = valid_pairing_code(presented).filter(|_| self.kind.has_short_code())
        else {
            return Redemption::Rejected;
        };
        let PairingChannel::Local { source } = channel else {
            return Redemption::LocalOnly;
        };
        if self.throttle.is_spent(source, now) {
            return Redemption::Throttled;
        }
        match self.take(|p| p.code.as_deref().is_some_and(|c| ct_eq(c, code))) {
            Some(intent) => {
                self.throttle.forgive(source);
                Redemption::Redeemed(intent)
            }
            None => {
                self.throttle.record(source, now);
                Redemption::Rejected
            }
        }
    }

    /// Remove the first pending pairing `matches` accepts, answering its intent.
    /// Every entry is compared, so timing says nothing about which one matched.
    fn take(&mut self, matches: impl Fn(&PendingPairing) -> bool) -> Option<PairingIntent> {
        let hits: Vec<bool> = self.pending.iter().map(matches).collect();
        let index = hits.iter().position(|hit| *hit)?;
        Some(self.pending.remove(index).intent)
    }

    fn drop_expired(&mut self, now: Instant) {
        self.pending
            .retain(|p| now.duration_since(p.minted) < PAIRING_TTL);
    }
}

/// Failures counted in one rolling window.
#[derive(Debug, Default, Clone, Copy)]
struct FailureWindow {
    opened: Option<Instant>,
    failed: u32,
}

impl FailureWindow {
    fn is_open(&self, now: Instant) -> bool {
        self.opened
            .is_some_and(|opened| now.duration_since(opened) < FAILURE_WINDOW)
    }

    fn is_spent(&self, now: Instant, cap: u32) -> bool {
        self.is_open(now) && self.failed >= cap
    }

    fn record(&mut self, now: Instant) {
        if self.is_open(now) {
            self.failed += 1;
        } else {
            *self = FailureWindow {
                opened: Some(now),
                failed: 1,
            };
        }
    }
}

/// The local short-code budget: one window per source, and a global backstop.
///
/// Memory stays bounded. A window is created only by a counted failure, and
/// nothing is counted once the backstop is spent.
#[derive(Debug, Default)]
struct ShortCodeThrottle {
    per_source: HashMap<IpAddr, FailureWindow>,
    global: FailureWindow,
}

impl ShortCodeThrottle {
    fn is_spent(&self, source: IpAddr, now: Instant) -> bool {
        self.global.is_spent(now, SHORT_CODE_FAILURES_GLOBAL)
            || self
                .per_source
                .get(&source)
                .is_some_and(|w| w.is_spent(now, SHORT_CODE_FAILURES_PER_SOURCE))
    }

    fn record(&mut self, source: IpAddr, now: Instant) {
        self.per_source.retain(|_, w| w.is_open(now));
        self.per_source.entry(source).or_default().record(now);
        self.global.record(now);
    }

    /// A correct code clears its source's budget, so a typo costs that person
    /// nothing on their next pairing.
    fn forgive(&mut self, source: IpAddr) {
        self.per_source.remove(&source);
    }
}

/// How far above the gateway's port the public listener sits: 5281 in dev,
/// 5282 packaged. `LUCIDOS_PUBLIC_PORT` overrides it, and `0` turns it off.
const PUBLIC_PORT_OFFSET: u16 = 30;

/// The public listener's port, read from this process's environment.
///
/// It serves the whole gateway, and every request it accepts is public. A
/// tunnel or reverse proxy points at it.
pub fn resolved_public_port(gateway_port: u16) -> Option<u16> {
    public_port(
        std::env::var("LUCIDOS_PUBLIC_PORT").ok().as_deref(),
        gateway_port,
    )
}

/// The public listener's address, read from this process's environment.
///
/// Loopback unless `LUCIDOS_PUBLIC_BIND_ADDR` names an IP, which a container
/// sets so its Ingress can reach the listener at the pod IP (ADR 0624).
pub fn resolved_public_bind_addr(port: u16) -> std::net::SocketAddr {
    crate::net_config::side_bind_addr(
        "LUCIDOS_PUBLIC_BIND_ADDR",
        std::env::var("LUCIDOS_PUBLIC_BIND_ADDR").ok().as_deref(),
        port,
    )
}

fn public_port(configured: Option<&str>, gateway_port: u16) -> Option<u16> {
    crate::net_config::side_port(
        "LUCIDOS_PUBLIC_PORT",
        configured,
        gateway_port,
        PUBLIC_PORT_OFFSET,
    )
}

/// How long a minted pairing stays redeemable, in seconds. Reported to the
/// client so the pairing screen can say it rather than guess.
pub fn pairing_ttl_secs() -> u64 {
    PAIRING_TTL.as_secs()
}

/// `N` bytes from the same entropy source as every other secret here.
fn random_bytes<const N: usize>() -> std::io::Result<[u8; N]> {
    use std::io::Read as _;
    let mut buf = [0u8; N];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(buf)
}

/// A long secret of `kind`: its prefix, then 26 base32 characters.
fn mint_secret(kind: PairingKind) -> std::io::Result<String> {
    let bytes = random_bytes::<17>()?;
    let mut out = String::with_capacity(kind.secret_prefix().len() + PAIRING_SECRET_CHARS);
    out.push_str(kind.secret_prefix());
    let (mut bits, mut held) = (0u32, 0u32);
    for byte in bytes {
        bits = (bits << 8) | u32::from(byte);
        held += 8;
        while held >= 5 && out.len() < kind.secret_prefix().len() + PAIRING_SECRET_CHARS {
            held -= 5;
            out.push(char::from(BASE32[((bits >> held) & 31) as usize]));
        }
        bits &= (1 << held) - 1;
    }
    Ok(out)
}

/// A zero-padded decimal short code.
fn mint_short_code() -> std::io::Result<String> {
    let value = u64::from_be_bytes(random_bytes::<8>()?) % 10u64.pow(PAIRING_CODE_DIGITS);
    Ok(format!(
        "{value:0width$}",
        width = PAIRING_CODE_DIGITS as usize
    ))
}

#[cfg(test)]
#[path = "pairing_tests.rs"]
mod tests;
