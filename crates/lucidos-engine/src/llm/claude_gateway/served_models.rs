//! The models a Claude gateway serves this account: its managed settings'
//! `availableModels` when they set one, else its `GET /v1/models` listing.
//!
//! A gateway allows each access group its own list of models. Lucidos sends
//! only an id the gateway allows, so a model outside the list is "not served"
//! before any request reaches the Messages API.

use std::time::{Duration, Instant};

use serde::Deserialize;

use super::gateway_managed_settings::GatewayManagedSettings;
use super::token_endpoint::{excerpt, http_client};
use super::{GatewayAccess, GatewayOrigin};
use crate::llm::anthropic_wire::parse_context_suffix;
use crate::llm::ModelNotServed;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// How long one listing answers before the gateway is asked again. An access
/// group's list changes rarely, and a stale one costs one refused turn.
const LISTING_TTL: Duration = Duration::from_secs(10 * 60);

/// Ids asked for per page, the Models API maximum.
const PAGE_LIMIT: &str = "1000";

/// Pages read before the listing stops. A gateway that never stops paging
/// would otherwise hold the turn forever.
const MAX_PAGES: usize = 10;

/// The gateway's listing, read on first use and kept for [`LISTING_TTL`].
pub(crate) struct ServedModels {
    origin: GatewayOrigin,
    listing: tokio::sync::Mutex<Option<Listing>>,
}

struct Listing {
    ids: Vec<String>,
    read_at: Instant,
}

#[derive(Deserialize)]
struct ModelsPage {
    data: Vec<ListedModel>,
    #[serde(default)]
    has_more: bool,
    last_id: Option<String>,
}

#[derive(Deserialize)]
struct ListedModel {
    id: String,
}

impl ServedModels {
    pub(crate) fn new(origin: GatewayOrigin) -> Self {
        Self {
            origin,
            listing: tokio::sync::Mutex::new(None),
        }
    }

    /// The id to send for `model`, or [`ModelNotServed`] when the gateway
    /// does not serve it. `policy` is the gateway's managed settings, whose
    /// `availableModels` replaces the listing when set. Also answers the
    /// bearer to send, renewed once when the listing rejected the first.
    ///
    /// The `[1m]` suffix is Lucidos's own, so it is matched without and kept on
    /// the answer. A model listed only as a dated snapshot is sent as that
    /// snapshot, which is the name the gateway listed.
    pub(crate) async fn wire_id(
        &self,
        model: &str,
        access: &dyn GatewayAccess,
        policy: Option<&GatewayManagedSettings>,
    ) -> Result<(String, String), BoxError> {
        let (base, one_million) = parse_context_suffix(model);
        let mut bearer = access.bearer().await?;
        let (ids, source) = match policy.and_then(GatewayManagedSettings::served_ids) {
            Some(allowed) => (allowed, "Its managed settings allow"),
            None => (self.ids(access, &mut bearer).await?, "It serves"),
        };
        let Some(listed) = listed_id(&ids, base) else {
            return Err(Box::new(ModelNotServed::new(format!(
                "The Claude gateway at {} does not serve {base} to this account. {source}: {}. \
                 Pick one of those, or ask the gateway's owners for access.",
                self.origin,
                ids.join(", ")
            ))));
        };
        if listed != base {
            crate::log!("[ClaudeGateway] Sending {base} as the listed snapshot {listed}");
        }
        let wire_id = match one_million {
            true => format!("{listed}[1m]"),
            false => listed.to_string(),
        };
        Ok((wire_id, bearer))
    }

    /// The listed ids, read again once the kept listing is older than
    /// [`LISTING_TTL`]. A `401` renews `bearer` once, as a `401` from the
    /// Messages API does.
    async fn ids(
        &self,
        access: &dyn GatewayAccess,
        bearer: &mut String,
    ) -> Result<Vec<String>, BoxError> {
        let mut listing = self.listing.lock().await;
        if let Some(kept) = listing
            .as_ref()
            .filter(|kept| kept.read_at.elapsed() < LISTING_TTL)
        {
            return Ok(kept.ids.clone());
        }
        let ids = match read_listing(&self.origin, bearer).await? {
            Listed::Ids(ids) => ids,
            Listed::Rejected(_) => {
                *bearer = access.renew_rejected(bearer).await?;
                match read_listing(&self.origin, bearer).await? {
                    Listed::Ids(ids) => ids,
                    Listed::Rejected(body) => return Err(super::unauthorized(&body).into()),
                }
            }
        };
        *listing = Some(Listing {
            ids: ids.clone(),
            read_at: Instant::now(),
        });
        Ok(ids)
    }
}

/// `model` itself when listed, else its latest dated snapshot
/// (`{model}-YYYYMMDD`), else `None`.
fn listed_id<'a>(ids: &'a [String], model: &str) -> Option<&'a str> {
    if let Some(exact) = ids.iter().find(|id| *id == model) {
        return Some(exact);
    }
    let is_snapshot_of_model = |id: &&String| {
        id.strip_prefix(model)
            .and_then(|rest| rest.strip_prefix('-'))
            .is_some_and(|stamp| stamp.len() == 8 && stamp.bytes().all(|b| b.is_ascii_digit()))
    };
    ids.iter()
        .filter(is_snapshot_of_model)
        .max()
        .map(String::as_str)
}

/// What one read of the listing found.
enum Listed {
    Ids(Vec<String>),
    /// The gateway answered `401`, with this body.
    Rejected(String),
}

/// Read every page of the gateway's `GET /v1/models`.
async fn read_listing(origin: &GatewayOrigin, bearer: &str) -> Result<Listed, BoxError> {
    let url = format!("{}/models", origin.api_base_url());
    let mut ids = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut request = http_client()
            .get(&url)
            .bearer_auth(bearer)
            .header(
                "anthropic-version",
                crate::llm::anthropic::chat::ANTHROPIC_VERSION,
            )
            .query(&[("limit", PAGE_LIMIT)]);
        if let Some(after) = &after {
            request = request.query(&[("after_id", after)]);
        }
        let resp = request
            .send()
            .await
            .map_err(|e| format!("Could not ask the Claude gateway which models it serves: {e}"))?;
        let status = resp.status();
        let body = resp.text().await?;
        if status.as_u16() == 401 {
            return Ok(Listed::Rejected(body));
        }
        if !status.is_success() {
            return Err(format!(
                "The Claude gateway answered {status} when asked which models it serves: {}",
                excerpt(&body)
            )
            .into());
        }
        let page: ModelsPage = serde_json::from_str(&body)
            .map_err(|e| format!("The Claude gateway's model listing is not readable: {e}"))?;
        ids.extend(page.data.into_iter().map(|model| model.id));
        match page.last_id.filter(|_| page.has_more) {
            Some(last) => after = Some(last),
            None => return Ok(Listed::Ids(ids)),
        }
    }
    crate::log!("[ClaudeGateway] {url} still had more models after {MAX_PAGES} pages");
    Ok(Listed::Ids(ids))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::claude_gateway::mock_gateway::MockGateway;

    /// Presents `gw-token`, and renews a rejected one as `gw-token-2`.
    struct FixedBearer;

    #[async_trait::async_trait]
    impl GatewayAccess for FixedBearer {
        async fn bearer(&self) -> Result<String, BoxError> {
            Ok("gw-token".into())
        }
        async fn renew_rejected(&self, _rejected: &str) -> Result<String, BoxError> {
            Ok("gw-token-2".into())
        }
        async fn managed_settings(&self) -> Result<Option<GatewayManagedSettings>, BoxError> {
            Ok(None)
        }
    }

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|id| id.to_string()).collect()
    }

    #[test]
    fn a_listed_id_matches_itself_first_then_its_latest_snapshot() {
        let listed = ids(&[
            "claude-opus-5-5",
            "claude-haiku-4-5-20250101",
            "claude-haiku-4-5-20251001",
        ]);
        assert_eq!(
            listed_id(&listed, "claude-opus-5-5"),
            Some("claude-opus-5-5")
        );
        assert_eq!(
            listed_id(&listed, "claude-haiku-4-5"),
            Some("claude-haiku-4-5-20251001")
        );
    }

    /// Only an eight-digit date is a snapshot. `claude-opus-5-5` is a newer
    /// model than `claude-opus-5`, not a snapshot of it.
    #[test]
    fn another_model_sharing_the_prefix_is_not_a_snapshot() {
        let listed = ids(&["claude-opus-5-5", "claude-opus-5-20261001x"]);
        assert_eq!(listed_id(&listed, "claude-opus-5"), None);
        assert_eq!(listed_id(&listed, "claude-sonnet-5"), None);
    }

    #[tokio::test]
    async fn the_1m_suffix_is_matched_without_and_kept_on_the_answer() {
        let gateway = MockGateway::start().await;
        gateway.list_models(&["claude-sonnet-5-5-20261001"], 1000);
        let served = ServedModels::new(gateway.origin.clone());
        assert_eq!(
            served
                .wire_id("claude-sonnet-5-5[1m]", &FixedBearer, None)
                .await
                .unwrap()
                .0,
            "claude-sonnet-5-5-20261001[1m]"
        );
    }

    /// The listing names what the account may use, so an unlisted model is
    /// refused as not served, naming what the gateway does serve.
    #[tokio::test]
    async fn an_unlisted_model_is_not_served() {
        let gateway = MockGateway::start().await;
        gateway.list_models(&["claude-haiku-5-5"], 1000);
        let served = ServedModels::new(gateway.origin.clone());
        let err = served
            .wire_id("claude-opus-5-5", &FixedBearer, None)
            .await
            .expect_err("the gateway did not list it");
        assert!(err.is::<ModelNotServed>(), "{err}");
        assert!(err.to_string().contains("claude-haiku-5-5"), "{err}");
    }

    /// Every page is read, each with the bearer and the Lucidos `User-Agent`,
    /// and one listing serves later calls.
    #[tokio::test]
    async fn the_listing_reads_every_page_once() {
        let gateway = MockGateway::start().await;
        gateway.list_models(&["a", "b", "c", "claude-opus-5-5", "e"], 2);
        let served = ServedModels::new(gateway.origin.clone());
        for _ in 0..2 {
            served
                .wire_id("claude-opus-5-5", &FixedBearer, None)
                .await
                .unwrap();
        }
        let seen: Vec<_> = gateway
            .seen()
            .into_iter()
            .filter(|s| s.path == "/v1/models")
            .collect();
        assert_eq!(seen.len(), 3, "three pages, read once");
        assert_eq!(seen[1].form.get("after_id").map(String::as_str), Some("b"));
        for request in &seen {
            assert_eq!(request.authorization.as_deref(), Some("Bearer gw-token"));
            assert!(request
                .user_agent
                .as_deref()
                .is_some_and(|ua| ua.starts_with("Lucidos/")));
        }
    }

    /// A policy allowlist replaces the listing: the gateway is not asked,
    /// and a model outside the allowlist is not served even when listed.
    #[tokio::test]
    async fn a_policy_allowlist_replaces_the_listing() {
        let gateway = MockGateway::start().await;
        gateway.list_models(&["claude-opus-5-5", "claude-haiku-5-5"], 1000);
        let served = ServedModels::new(gateway.origin.clone());
        let policy = GatewayManagedSettings::from_document(&serde_json::json!({
            "availableModels": ["claude-haiku-5-5[1m]", "claude-sonnet-5-5@20261001"]
        }));
        let err = served
            .wire_id("claude-opus-5-5", &FixedBearer, Some(&policy))
            .await
            .expect_err("listed, but outside the allowlist");
        assert!(err.is::<ModelNotServed>(), "{err}");
        assert!(err.to_string().contains("managed settings allow"), "{err}");
        assert_eq!(
            served
                .wire_id("claude-haiku-5-5[1m]", &FixedBearer, Some(&policy))
                .await
                .unwrap()
                .0,
            "claude-haiku-5-5[1m]"
        );
        assert!(
            served
                .wire_id("claude-sonnet-5-5", &FixedBearer, Some(&policy))
                .await
                .is_err(),
            "an id@alias entry is dropped from the served list"
        );
        assert!(gateway.paths().is_empty(), "the listing is never read");
    }

    #[tokio::test]
    async fn a_refused_listing_says_why() {
        let served = ServedModels::new(GatewayOrigin::parse("http://127.0.0.1:9").unwrap());
        let err = served
            .wire_id("claude-opus-5-5", &FixedBearer, None)
            .await
            .expect_err("nothing listens there");
        assert!(!err.is::<ModelNotServed>(), "{err}");
        assert!(err.to_string().contains("which models it serves"), "{err}");
    }

    /// A `401` on the listing renews the bearer once, and the call goes on
    /// with the renewed one.
    #[tokio::test]
    async fn a_rejected_listing_renews_the_bearer() {
        let gateway = MockGateway::start().await;
        gateway.list_models(&["claude-opus-5-5"], 1000);
        gateway.reject_listing_for("gw-token");
        let served = ServedModels::new(gateway.origin.clone());
        let (model, bearer) = served
            .wire_id("claude-opus-5-5", &FixedBearer, None)
            .await
            .unwrap();
        assert_eq!(
            (model.as_str(), bearer.as_str()),
            ("claude-opus-5-5", "gw-token-2")
        );
    }
}
