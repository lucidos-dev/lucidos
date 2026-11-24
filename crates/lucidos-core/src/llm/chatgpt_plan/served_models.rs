//! The models the signed-in ChatGPT plan serves: its `GET /v1/models` slugs
//! with `visibility == "list"`.
//!
//! The plan route sends only a listed slug. A model row the plan does not list
//! is "not served" before any request spends the plan.

use std::time::{Duration, Instant};

use serde::Deserialize;

use super::auth_server::http_client;
use super::{ChatGptPlanError, ChatGptPlanTokens};
use crate::llm::ModelNotServed;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// How long one listing answers before OpenAI is asked again.
const LISTING_TTL: Duration = Duration::from_secs(10 * 60);

/// The plan's listing, read on first use and kept for [`LISTING_TTL`].
pub(crate) struct ServedModels {
    models_url: String,
    listing: tokio::sync::Mutex<Option<Listing>>,
}

struct Listing {
    slugs: Vec<String>,
    read_at: Instant,
}

/// The plan's listing shape, which differs from the platform's `data` array.
#[derive(Deserialize)]
struct ModelsReply {
    models: Vec<ListedModel>,
}

#[derive(Deserialize)]
struct ListedModel {
    slug: String,
    #[serde(default)]
    visibility: Option<String>,
}

/// What one read of the listing found.
enum Listed {
    Slugs(Vec<String>),
    /// OpenAI answered `401`.
    Rejected,
}

impl ServedModels {
    pub(crate) fn new(models_url: String) -> Self {
        Self {
            models_url,
            listing: tokio::sync::Mutex::new(None),
        }
    }

    /// Refuse `model` with [`ModelNotServed`] unless the plan lists it.
    /// `bearer` is renewed once when the listing rejected it, so the request
    /// that follows sends the renewed one.
    pub(crate) async fn check(
        &self,
        model: &str,
        tokens: &dyn ChatGptPlanTokens,
        bearer: &mut String,
    ) -> Result<(), BoxError> {
        let slugs = self.slugs(tokens, bearer).await?;
        if slugs.iter().any(|slug| slug == model) {
            return Ok(());
        }
        Err(Box::new(ModelNotServed::new(format!(
            "Your ChatGPT plan does not serve {model} to Lucidos. It serves: {}. Pick one of \
             those, or switch this model to another route.",
            slugs.join(", ")
        ))))
    }

    async fn slugs(
        &self,
        tokens: &dyn ChatGptPlanTokens,
        bearer: &mut String,
    ) -> Result<Vec<String>, BoxError> {
        let mut listing = self.listing.lock().await;
        if let Some(kept) = listing
            .as_ref()
            .filter(|kept| kept.read_at.elapsed() < LISTING_TTL)
        {
            return Ok(kept.slugs.clone());
        }
        let slugs = match read_listing(&self.models_url, bearer).await? {
            Listed::Slugs(slugs) => slugs,
            Listed::Rejected => {
                *bearer = tokens.after_rejection(bearer).await?;
                match read_listing(&self.models_url, bearer).await? {
                    Listed::Slugs(slugs) => slugs,
                    Listed::Rejected => {
                        return Err(
                            "OpenAI refused the ChatGPT sign-in when asked which models \
                                    the plan serves. Sign in with ChatGPT again."
                                .into(),
                        )
                    }
                }
            }
        };
        *listing = Some(Listing {
            slugs: slugs.clone(),
            read_at: Instant::now(),
        });
        Ok(slugs)
    }
}

async fn read_listing(models_url: &str, bearer: &str) -> Result<Listed, BoxError> {
    let resp = http_client()
        .get(models_url)
        .bearer_auth(bearer)
        .send()
        .await
        .map_err(|e| format!("Could not ask OpenAI which models the ChatGPT plan serves: {e}"))?;
    let status = resp.status();
    let request_id = crate::llm::call_trace::request_id_of(resp.headers());
    let body = resp.text().await?;
    if status.as_u16() == 401 {
        return Ok(Listed::Rejected);
    }
    if !status.is_success() {
        // A plan code (a usage limit, an ineligible plan) fails typed, so the
        // caller can start the limit pause and say what to do.
        if let Some(plan_error) = ChatGptPlanError::from_http(status.as_u16(), &body, request_id) {
            return Err(Box::new(plan_error));
        }
        return Err(format!(
            "OpenAI answered {status} when asked which models the ChatGPT plan serves: {}",
            &body[..body.floor_char_boundary(300)]
        )
        .into());
    }
    let reply: ModelsReply = serde_json::from_str(&body)
        .map_err(|e| format!("The ChatGPT plan's model listing is not readable: {e}"))?;
    Ok(Listed::Slugs(
        reply
            .models
            .into_iter()
            .filter(|model| model.visibility.as_deref() == Some("list"))
            .map(|model| model.slug)
            .collect(),
    ))
}
