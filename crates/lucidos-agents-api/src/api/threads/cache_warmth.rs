use axum::{
    extract::{Path, State},
    Json,
};
use serde::Serialize;
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::AppState;
use crate::engine::cache_ttl::{CacheLifetime, CacheTtl};
use crate::engine::switch_estimate::{cache_remaining_and_ttl, CacheRemaining};
use crate::engine::thread_events::CacheKeepAliveStatus;

#[derive(Debug, Serialize)]
pub(in crate::api) struct CacheWarmth {
    warmth: Warmth,
    remaining_secs: Option<i64>,
    keep_alive: Option<CacheKeepAliveStatus>,
    ttl: Option<WarmthTtl>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
struct WarmthTtl {
    lifetime: CacheLifetime,
    evidence: TtlEvidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum TtlEvidence {
    Reported,
    Requested,
}

impl WarmthTtl {
    fn of(ttl: CacheTtl) -> Option<Self> {
        let (lifetime, evidence) = match ttl {
            CacheTtl::Reported(lifetime) => (lifetime, TtlEvidence::Reported),
            CacheTtl::Requested(lifetime) => (lifetime, TtlEvidence::Requested),
            CacheTtl::Unknown => return None,
        };
        Some(Self { lifetime, evidence })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Warmth {
    Warm,
    Cold,
    Unknown,
}

pub(in crate::api) async fn get_cache_warmth(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
) -> Result<Json<CacheWarmth>, ApiError> {
    let thread_id = Uuid::parse_str(&thread_id)
        .map_err(|e| ApiError::bad_request(format!("Invalid thread id: {e}")))?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM thread_summaries WHERE thread_id = $1)")
            .bind(thread_id)
            .fetch_one(&state.pool)
            .await
            .map_err(ApiError::db)?;
    if !exists {
        return Err(ApiError::not_found(format!(
            "Thread {thread_id} does not exist"
        )));
    }
    let (remaining, ttl) = cache_remaining_and_ttl(&state.pool, thread_id)
        .await
        .map_err(ApiError::db)?;
    let (warmth, remaining_secs, ttl) = match remaining {
        CacheRemaining::Warm(secs) => (Warmth::Warm, Some(secs), WarmthTtl::of(ttl)),
        CacheRemaining::Cold => (Warmth::Cold, None, None),
        CacheRemaining::Unknown => (Warmth::Unknown, None, None),
    };
    let keep_alive = match warmth {
        Warmth::Warm | Warmth::Unknown => Some(
            state
                .engine
                .warm_line_keep_alive(thread_id)
                .await
                .map_err(ApiError::db)?,
        ),
        Warmth::Cold => None,
    };
    Ok(Json(CacheWarmth {
        warmth,
        remaining_secs,
        keep_alive,
        ttl,
    }))
}
