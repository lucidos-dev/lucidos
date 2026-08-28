//! Inbound webhooks: configuration, and the verification a delivery must pass.
//!
//! Full design:
//! `docs/plans/2026-08-19-webhooks-and-engines-off-the-network.md`.
//!
//! # Signature checking is data, not code
//!
//! [`HmacConfig`] describes a sender's scheme in fields: which header carries
//! the signature, how to pull it out, what string is signed, and how the digest
//! is encoded. GitHub, Stripe and Slack are all expressible, so none of the
//! three needs engine code, and a fourth sender is a config change.
//!
//! The secret is never here. `credential` names a row in the `credentials`
//! table, which stays the only home for the value.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

pub use lucidos_local_token::ct_eq;

use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;

/// One configured webhook, as stored. Never carries a secret: `token_hash` is a
/// digest, and [`HmacConfig::credential`] is a name.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Webhook {
    pub id: Uuid,
    pub name: String,
    pub event_type: String,
    #[serde(skip_serializing)]
    pub token_hash: Option<String>,
    pub hmac: Option<HmacConfig>,
    /// `None` means the hook does not dedupe, which is the default. Every
    /// arrival then emits, so the log keeps the sender's retries.
    pub dedupe: Option<DedupeConfig>,
    /// Request headers copied into the event payload. An allow-list, because
    /// the events table is append-only and a carried secret is a permanent one.
    pub headers: Vec<String>,
    pub enabled: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// When a delivery last verified and emitted.
    pub last_accepted_at: Option<chrono::DateTime<chrono::Utc>>,
    /// When a delivery last arrived and was turned away.
    ///
    /// This is the diagnostic half. Silence alone cannot tell a rotated secret
    /// from a dead ingress, and a refusal can.
    pub last_refused_at: Option<chrono::DateTime<chrono::Utc>>,
    /// What [`DeliveryRefusal::reason`] said about that refusal.
    pub last_refusal_reason: Option<String>,
}

/// Everything about a webhook except its identity: how it authenticates, and
/// what it does with a delivery once one arrives.
///
/// Grouped because the three travel together and are the whole of what
/// distinguishes one hook from another. Separately they push `create` past the
/// argument count anyone can read at a call site.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WebhookConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hmac: Option<HmacConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dedupe: Option<DedupeConfig>,
    #[serde(default)]
    pub headers: Vec<String>,
}

/// A change to an existing webhook. Every field is optional, and `None` keeps
/// the stored value.
///
/// `dedupe` needs no "clear it" variant, so it stays free of the
/// `Option<Option<_>>` an optional JSONB column otherwise wants: a config with
/// `window_secs: 0` switches deduping off.
///
/// `hmac` has no such off value, so it takes a named three-state instead. The
/// same reasoning, one step further: a shape nobody has to decode beats a
/// nested option.
#[derive(Debug, Clone, Default)]
pub struct WebhookPatch {
    pub name: Option<String>,
    pub event_type: Option<String>,
    pub enabled: Option<bool>,
    pub hmac: HmacChange,
    pub dedupe: Option<DedupeConfig>,
    pub headers: Option<Vec<String>>,
}

/// What an update does to a hook's signature config.
///
/// # A hook carries one verifier kind, so a change to either moves both
///
/// [`WebhookStore::create`] mints a token only for an unsigned hook, because
/// [`verify`] requires every verifier a row carries and no signing sender
/// attaches a bearer token. An update has to hold the same line from both
/// sides, or it hands the user a hook that refuses everything:
///
/// - [`Self::Set`] drops the token. A hook that gained a signature and kept its
///   token would refuse every delivery GitHub, Slack or Stripe can send.
/// - [`Self::Clear`] mints one. The table's CHECK says a hook has at least one
///   verifier, so unsigned with no token is a row that cannot exist. Refusing
///   the transition was the alternative, and it sends the user back to delete
///   and recreate, which changes the delivery URL and breaks the sender. That
///   is the whole reason `hmac` became editable, so minting wins.
///
/// The minted token is returned once, on the contract `create` already has.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum HmacChange {
    /// Keep the stored config, whatever it is.
    #[default]
    Keep,
    /// Sign from now on, with this config.
    Set(HmacConfig),
    /// Stop signing, and go back to a bearer token.
    Clear,
}

/// Absent keeps, an object sets, and `null` clears.
///
/// Hand-written so the request DTO needs no `Option<Option<_>>` either.
/// `#[serde(default)]` answers the absent case, and this answers the other two.
impl<'de> Deserialize<'de> for HmacChange {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match Option::<HmacConfig>::deserialize(deserializer)? {
            Some(cfg) => Self::Set(cfg),
            None => Self::Clear,
        })
    }
}

/// How a hook recognises a delivery it has already emitted.
///
/// Named as data, the same shape [`HmacConfig`] uses: the sender says which
/// header carries its delivery id, and no provider needs engine code.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DedupeConfig {
    /// Header carrying the sender's own delivery id, such as
    /// `X-GitHub-Delivery`. Absent here, or absent from a given request, and
    /// the key is a digest of the body instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    /// How long a claim holds a key. `0` switches deduping off, which is also
    /// how `update` clears the setting.
    #[serde(default = "default_window_secs")]
    pub window_secs: i64,
}

fn default_window_secs() -> i64 {
    3600
}

/// The key a delivery is deduped on.
///
/// A digest either way, so the ledger stores one fixed-length value rather than
/// whatever a public caller put in a header. The two sources are prefixed
/// apart, so a body can never key the same claim as a header value.
///
/// Falling back is safe because this key authenticates nothing. It decides only
/// whether this delivery has been seen, and that decision runs after
/// [`verify`].
pub fn dedupe_key(header_value: Option<&str>, body: &str) -> String {
    match header_value {
        Some(value) => digest(&format!("header:{value}")),
        None => digest(&format!("body:{body}")),
    }
}

/// Which digest a sender signs with.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum HmacAlgorithm {
    #[default]
    Sha256,
    Sha1,
}

/// How the digest is written into the header.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DigestEncoding {
    #[default]
    Hex,
    Base64,
}

/// A sender's signature scheme, in fields.
///
/// Worked examples, all three real:
///
/// | Sender | header | prefix / key | template |
/// |---|---|---|---|
/// | GitHub | `X-Hub-Signature-256` | `prefix: "sha256="` | `{body}` |
/// | Slack | `X-Slack-Signature` | `prefix: "v0="` | `v0:{timestamp}:{body}` |
/// | Stripe | `Stripe-Signature` | `signature_key: "v1"` | `{timestamp}.{body}` |
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HmacConfig {
    /// The credential's `service_name`. The secret itself lives there.
    pub credential: String,
    /// Header carrying the signature.
    pub signature_header: String,
    #[serde(default)]
    pub algorithm: HmacAlgorithm,
    #[serde(default)]
    pub encoding: DigestEncoding,
    /// Literal prefix to strip off the header value, such as `sha256=`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    /// Key to read out of a comma-separated `k=v` header, such as Stripe's
    /// `v1`. Mutually useful with `prefix`, and checked first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature_key: Option<String>,
    /// Header carrying the signed timestamp, when the scheme has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp_header: Option<String>,
    /// Key holding the timestamp inside the SIGNATURE header, for a sender that
    /// packs both into one, as Stripe does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp_key: Option<String>,
    /// The string that gets signed. `{body}` and `{timestamp}` are substituted.
    /// Defaults to the body alone.
    #[serde(default = "default_template")]
    pub template: String,
    /// How far the signed timestamp may be from now, in seconds. `None` skips
    /// the check, which is right only for a scheme with no timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerance_secs: Option<i64>,
}

fn default_template() -> String {
    "{body}".to_string()
}

/// Why a delivery was refused. Every arm answers 401, so this exists to be
/// logged and tested rather than to be branched on by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryRefusal {
    /// No bearer token, or the wrong one.
    Token,
    /// The signature header is missing or unparseable.
    SignatureMissing,
    /// The signature did not match.
    SignatureMismatch,
    /// The signed timestamp is outside the configured tolerance.
    TimestampOutsideTolerance,
    /// The named credential does not exist, so nothing can be verified.
    CredentialMissing,
}

impl DeliveryRefusal {
    /// What to write in the log. Never returned to the caller: a public
    /// endpoint that says WHY it refused is a hint to whoever is guessing.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Token => "bearer token did not match",
            Self::SignatureMissing => "signature header missing or unparseable",
            Self::SignatureMismatch => "signature did not match",
            Self::TimestampOutsideTolerance => "signed timestamp is too old or too far ahead",
            Self::CredentialMissing => "the configured credential does not exist",
        }
    }
}

/// Lowercase-hex SHA-256 of `value`, the form `token_hash` stores.
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

/// A fresh webhook token: 32 bytes of entropy, lowercase hex.
///
/// Both this and [`ct_eq`] come from `lucidos-local-token` rather than being
/// written again here. That crate exists because four hand-copies of a secret's
/// minting and comparison drifted, and a stale copy is a caller that silently
/// cannot authenticate.
pub fn mint_token() -> std::io::Result<String> {
    lucidos_local_token::mint_hex(32)
}

/// The bearer token a request presented, if it presented one properly.
pub fn presented_bearer(authorization: Option<&str>) -> Option<&str> {
    let value = authorization?.trim();
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

/// Pull the signature out of a header value, per the configured scheme.
///
/// `signature_key` wins when set, for a sender packing several fields into one
/// header. Otherwise a `prefix` is stripped, and a value that lacks the prefix
/// is refused rather than passed through: matching a bare digest against a
/// prefixed scheme would accept a signature computed for something else.
pub fn extract_signature<'a>(cfg: &HmacConfig, header_value: &'a str) -> Option<&'a str> {
    let header_value = header_value.trim();
    if let Some(key) = cfg.signature_key.as_deref() {
        return field_from_pairs(header_value, key);
    }
    match cfg.prefix.as_deref() {
        Some(prefix) if !prefix.is_empty() => header_value.strip_prefix(prefix),
        _ => Some(header_value),
    }
    .map(str::trim)
    .filter(|s| !s.is_empty())
}

/// The timestamp a request signed, from its own header or from a key inside the
/// signature header.
pub fn extract_timestamp<'a>(
    cfg: &HmacConfig,
    signature_header_value: &'a str,
    timestamp_header_value: Option<&'a str>,
) -> Option<&'a str> {
    if let Some(key) = cfg.timestamp_key.as_deref() {
        return field_from_pairs(signature_header_value.trim(), key);
    }
    timestamp_header_value
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// One value out of a comma-separated `k=v` list, matched on the whole key.
fn field_from_pairs<'a>(raw: &'a str, key: &str) -> Option<&'a str> {
    raw.split(',')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.trim())
        .filter(|v| !v.is_empty())
}

/// Build the string the sender signed.
pub fn canonical_string(template: &str, timestamp: Option<&str>, body: &str) -> String {
    template
        .replace("{timestamp}", timestamp.unwrap_or_default())
        .replace("{body}", body)
}

/// Compute the expected signature for `canonical`, encoded as the sender writes
/// it.
pub fn sign(cfg: &HmacConfig, secret: &str, canonical: &str) -> String {
    use base64::Engine as _;
    use hmac::{Hmac, Mac};
    let raw: Vec<u8> = match cfg.algorithm {
        HmacAlgorithm::Sha256 => {
            let mut mac = <Hmac<sha2::Sha256> as Mac>::new_from_slice(secret.as_bytes())
                .expect("hmac takes a key of any length");
            mac.update(canonical.as_bytes());
            mac.finalize().into_bytes().to_vec()
        }
        HmacAlgorithm::Sha1 => {
            let mut mac = <Hmac<sha1::Sha1> as Mac>::new_from_slice(secret.as_bytes())
                .expect("hmac takes a key of any length");
            mac.update(canonical.as_bytes());
            mac.finalize().into_bytes().to_vec()
        }
    };
    match cfg.encoding {
        DigestEncoding::Hex => {
            let mut s = String::with_capacity(raw.len() * 2);
            for b in &raw {
                use std::fmt::Write as _;
                let _ = write!(s, "{b:02x}");
            }
            s
        }
        DigestEncoding::Base64 => base64::engine::general_purpose::STANDARD.encode(&raw),
    }
}

/// Is a signed timestamp close enough to now?
///
/// A missing tolerance means the scheme carries no timestamp, so there is
/// nothing to check. A tolerance with an unparseable timestamp is a refusal:
/// the configuration asked for a replay window and did not get one.
///
/// The distance is computed unsigned, because the timestamp is a header a
/// public caller writes. A plain `(now - parsed).abs()` overflows on
/// `i64::MIN`. That panics in a debug build and silently wraps in a release
/// one, so a caller would pick which of those this endpoint does.
pub fn timestamp_within_tolerance(
    tolerance_secs: Option<i64>,
    timestamp: Option<&str>,
    now_unix: i64,
) -> bool {
    let Some(tolerance) = tolerance_secs else {
        return true;
    };
    // A negative tolerance admits nothing, and saying so here keeps the
    // comparison below in one unsigned domain.
    let Ok(tolerance) = u64::try_from(tolerance) else {
        return false;
    };
    let Some(parsed) = timestamp.and_then(|t| t.trim().parse::<i64>().ok()) else {
        return false;
    };
    now_unix.abs_diff(parsed) <= tolerance
}

/// Everything a delivery presented, gathered before any of it is trusted.
pub struct PresentedDelivery<'a> {
    pub authorization: Option<&'a str>,
    pub signature_header: Option<&'a str>,
    pub timestamp_header: Option<&'a str>,
    /// The request body, verbatim. Signed as-is, so it is never reserialized.
    pub body: &'a str,
    pub now_unix: i64,
}

/// Decide whether a delivery may emit this webhook's event.
///
/// Every configured verifier must pass, and a webhook always has at least one
/// (the table's CHECK constraint is the floor). `secret` is the resolved
/// credential value, `None` when the named credential is gone.
pub fn verify(
    hook: &Webhook,
    presented: &PresentedDelivery<'_>,
    secret: Option<&str>,
) -> Result<(), DeliveryRefusal> {
    if let Some(expected) = hook.token_hash.as_deref() {
        let token = presented_bearer(presented.authorization).ok_or(DeliveryRefusal::Token)?;
        if !ct_eq(&digest(token), expected) {
            return Err(DeliveryRefusal::Token);
        }
    }

    let Some(cfg) = hook.hmac.as_ref() else {
        return Ok(());
    };
    let secret = secret.ok_or(DeliveryRefusal::CredentialMissing)?;
    let header = presented
        .signature_header
        .ok_or(DeliveryRefusal::SignatureMissing)?;
    let signature = extract_signature(cfg, header).ok_or(DeliveryRefusal::SignatureMissing)?;
    let timestamp = extract_timestamp(cfg, header, presented.timestamp_header);
    if !timestamp_within_tolerance(cfg.tolerance_secs, timestamp, presented.now_unix) {
        return Err(DeliveryRefusal::TimestampOutsideTolerance);
    }
    let canonical = canonical_string(&cfg.template, timestamp, presented.body);
    if !ct_eq(&sign(cfg, secret, &canonical), signature) {
        return Err(DeliveryRefusal::SignatureMismatch);
    }
    Ok(())
}

/// The columns every read selects, in the order [`row_to_webhook`] unpacks.
const WEBHOOK_COLUMNS: &str = "id, name, event_type, token_hash, hmac, dedupe, headers, \
                               enabled, created_at, updated_at, last_accepted_at, \
                               last_refused_at, last_refusal_reason";

type WebhookRow = (
    Uuid,
    String,
    String,
    Option<String>,
    Option<serde_json::Value>,
    Option<serde_json::Value>,
    Vec<String>,
    bool,
    chrono::DateTime<chrono::Utc>,
    chrono::DateTime<chrono::Utc>,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<String>,
);

fn row_to_webhook(row: WebhookRow) -> Result<Webhook, Box<dyn std::error::Error + Send + Sync>> {
    let (
        id,
        name,
        event_type,
        token_hash,
        hmac,
        dedupe,
        headers,
        enabled,
        created_at,
        updated_at,
        last_accepted_at,
        last_refused_at,
        last_refusal_reason,
    ) = row;
    Ok(Webhook {
        id,
        name,
        event_type,
        token_hash,
        hmac: hmac.map(serde_json::from_value).transpose()?,
        dedupe: dedupe.map(serde_json::from_value).transpose()?,
        headers,
        enabled,
        created_at,
        updated_at,
        last_accepted_at,
        last_refused_at,
        last_refusal_reason,
    })
}

/// The registry of inbound webhooks.
///
/// **No caller can skip the event.** Create, update and delete each emit from
/// here rather than from their call sites, so every mutation path announces by
/// construction. Registered in `core::announced_surfaces`.
pub struct WebhookStore;

impl WebhookStore {
    pub async fn list(
        pool: &PgPool,
    ) -> Result<Vec<Webhook>, Box<dyn std::error::Error + Send + Sync>> {
        let rows: Vec<WebhookRow> = sqlx::query_as(&format!(
            "SELECT {WEBHOOK_COLUMNS} FROM webhooks ORDER BY created_at"
        ))
        .fetch_all(pool)
        .await?;
        rows.into_iter().map(row_to_webhook).collect()
    }

    pub async fn get(
        pool: &PgPool,
        id: Uuid,
    ) -> Result<Option<Webhook>, Box<dyn std::error::Error + Send + Sync>> {
        let row: Option<WebhookRow> = sqlx::query_as(&format!(
            "SELECT {WEBHOOK_COLUMNS} FROM webhooks WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(pool)
        .await?;
        row.map(row_to_webhook).transpose()
    }

    /// Create a webhook, and mint a token for it unless it signs instead.
    ///
    /// **A signed hook gets no token, and that is what makes it usable.** A
    /// sender like GitHub cannot attach one, so a hook holding both verifiers
    /// would refuse every real delivery. `verify` requires each verifier the
    /// row carries, so minting a token here would be pinning a credential the
    /// sender has no way to present.
    ///
    /// `Ok((hook, None))` therefore means signature-only. `Some(token)` is the
    /// one time that token exists in readable form.
    pub async fn create(
        pool: &PgPool,
        bus: &EventBus,
        name: &str,
        event_type: &str,
        config: WebhookConfig,
        actor: Option<MessageOrigin>,
    ) -> Result<(Webhook, Option<String>), Box<dyn std::error::Error + Send + Sync>> {
        let WebhookConfig {
            hmac,
            dedupe,
            headers,
        } = config;
        let token = match hmac {
            Some(_) => None,
            None => Some(mint_token()?),
        };
        let id = Uuid::new_v4();
        let hmac_json = hmac.as_ref().map(serde_json::to_value).transpose()?;
        let dedupe_json = dedupe.as_ref().map(serde_json::to_value).transpose()?;
        let row: WebhookRow = sqlx::query_as(&format!(
            "INSERT INTO webhooks (id, name, event_type, token_hash, hmac, dedupe, headers) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {WEBHOOK_COLUMNS}"
        ))
        .bind(id)
        .bind(name)
        .bind(event_type)
        .bind(token.as_deref().map(digest))
        .bind(hmac_json)
        .bind(dedupe_json)
        .bind(&headers)
        .fetch_one(pool)
        .await?;
        let hook = row_to_webhook(row)?;
        bus.emit(BusEvent::System(SystemEvent::WebhookCreated {
            webhook_id: hook.id.to_string(),
            name: hook.name.clone(),
            event_type: hook.event_type.clone(),
            signed: hook.hmac.is_some(),
            actor,
        }))
        .await?;
        Ok((hook, token))
    }

    /// Change a webhook. `Ok(None)` means no webhook has that id.
    ///
    /// The token comes back for the one update that mints one, which is the
    /// clear described on [`HmacChange`]. Every other update returns `None`
    /// beside the hook, since no token changed hands.
    pub async fn update(
        pool: &PgPool,
        bus: &EventBus,
        id: Uuid,
        patch: WebhookPatch,
        actor: Option<MessageOrigin>,
    ) -> Result<Option<(Webhook, Option<String>)>, Box<dyn std::error::Error + Send + Sync>> {
        let WebhookPatch {
            name,
            event_type,
            enabled,
            hmac,
            dedupe,
            headers,
        } = patch;
        let dedupe_json = dedupe.as_ref().map(serde_json::to_value).transpose()?;
        // The verifier moves as one. A single flag writes `hmac` and
        // `token_hash` together, so no combination of arguments can leave a row
        // carrying both verifiers or neither.
        let (verifier_moves, hmac_json, token) = match &hmac {
            HmacChange::Keep => (false, None, None),
            HmacChange::Set(cfg) => (true, Some(serde_json::to_value(cfg)?), None),
            HmacChange::Clear => (true, None, Some(mint_token()?)),
        };
        let row: Option<WebhookRow> = sqlx::query_as(&format!(
            "UPDATE webhooks SET name = COALESCE($2, name), \
             event_type = COALESCE($3, event_type), enabled = COALESCE($4, enabled), \
             dedupe = COALESCE($5, dedupe), headers = COALESCE($6, headers), \
             hmac = CASE WHEN $7 THEN $8 ELSE hmac END, \
             token_hash = CASE WHEN $7 THEN $9 ELSE token_hash END, \
             updated_at = NOW() WHERE id = $1 RETURNING {WEBHOOK_COLUMNS}"
        ))
        .bind(id)
        .bind(&name)
        .bind(&event_type)
        .bind(enabled)
        .bind(dedupe_json)
        .bind(&headers)
        .bind(verifier_moves)
        .bind(hmac_json)
        .bind(token.as_deref().map(digest))
        .fetch_optional(pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let hook = row_to_webhook(row)?;
        bus.emit(BusEvent::System(SystemEvent::WebhookUpdated {
            webhook_id: hook.id.to_string(),
            name: hook.name.clone(),
            event_type: hook.event_type.clone(),
            enabled: hook.enabled,
            signed: hook.hmac.is_some(),
            actor,
        }))
        .await?;
        Ok(Some((hook, token)))
    }

    /// Stamp that a delivery verified and emitted.
    ///
    /// An observation, so it emits nothing. `updated_at` stays where it is:
    /// nobody changed the hook, and moving it would make every delivery look
    /// like an edit.
    pub async fn record_accepted(
        pool: &PgPool,
        id: Uuid,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        sqlx::query("UPDATE webhooks SET last_accepted_at = NOW() WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(())
    }

    /// Stamp that a delivery arrived and was turned away, and why.
    ///
    /// `reason` is a [`DeliveryRefusal::reason`] string. It reaches the
    /// workspace owner's page and never the sender.
    pub async fn record_refused(
        pool: &PgPool,
        id: Uuid,
        reason: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        sqlx::query(
            "UPDATE webhooks SET last_refused_at = NOW(), last_refusal_reason = $2 WHERE id = $1",
        )
        .bind(id)
        .bind(reason)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Delete a webhook. `Ok(false)` means no webhook had that id.
    pub async fn delete(
        pool: &PgPool,
        bus: &EventBus,
        id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let name: Option<(String,)> =
            sqlx::query_as("DELETE FROM webhooks WHERE id = $1 RETURNING name")
                .bind(id)
                .fetch_optional(pool)
                .await?;
        let Some((name,)) = name else {
            return Ok(false);
        };
        bus.emit(BusEvent::System(SystemEvent::WebhookDeleted {
            webhook_id: id.to_string(),
            name,
            actor,
        }))
        .await?;
        Ok(true)
    }
}

#[cfg(test)]
#[path = "webhooks_tests.rs"]
mod tests;
