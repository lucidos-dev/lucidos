//! Every model call an arm's workspace made, by why it was made.
//!
//! A thread row prices what its own thread had captured when it settled. This
//! reads the whole arm database at report time instead. So work that landed
//! later or on another thread counts too: the compactor, a sub-thread, a
//! trigger run. ADR 0362 asks what the Tree memory module costs in model
//! calls, compactor calls included, and this is that number for every arm.

use sqlx::{PgPool, Row};

use crate::config::ModelPrice;
use crate::metrics::{self, TokenCounts};

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The purpose a capture without one records: the agent's own round trip.
pub const TURN_PURPOSE: &str = "turn";

/// One purpose and model's calls over a whole arm.
#[derive(Debug, Clone, PartialEq)]
pub struct PurposeCalls {
    /// The engine's `ContextPurpose`, snake_case on the wire.
    pub purpose: String,
    pub model: String,
    pub calls: i64,
    pub counts: TokenCounts,
    pub usd: f64,
}

/// Summary tree nodes, by whether a model wrote them.
///
/// A node whose source already fit is stored verbatim with no call. So the
/// written count is a floor on compactor calls. The captured calls are the
/// exact figure wherever a thread anchored them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TreeNodes {
    pub written: i64,
    pub free: i64,
}

/// Every capture in the arm's database, grouped by purpose and model, priced.
pub async fn by_purpose(pool: &PgPool, prices: &[ModelPrice]) -> Fallible<Vec<PurposeCalls>> {
    // `sum()` over bigint returns NUMERIC, so each total is cast back, as in
    // `metrics::thread_tokens`.
    let rows = sqlx::query(
        "SELECT COALESCE(payload->>'purpose', $1)                                 AS purpose, \
                COALESCE(payload->>'model', '')                                   AS model, \
                count(*)::int8                                                    AS calls, \
                sum((payload->'usage'->>'cache_creation_tokens')::bigint)::bigint AS cache_creation, \
                sum((payload->'usage'->>'cache_read_tokens')::bigint)::bigint     AS cache_read, \
                sum((payload->'usage'->>'input_tokens')::bigint)::bigint          AS input_total, \
                sum((payload->'usage'->>'output_tokens')::bigint)::bigint         AS output_tokens \
           FROM events \
          WHERE event_type = 'ContextCaptured' \
          GROUP BY 1, 2 ORDER BY 1, 2",
    )
    .bind(TURN_PURPOSE)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            let purpose: String = row.try_get("purpose")?;
            let model: String = row.try_get("model")?;
            let counts = TokenCounts {
                cache_creation: row
                    .try_get::<Option<i64>, _>("cache_creation")?
                    .unwrap_or(0),
                cache_read: row.try_get::<Option<i64>, _>("cache_read")?.unwrap_or(0),
                input_total: row.try_get::<Option<i64>, _>("input_total")?.unwrap_or(0),
                output_tokens: row.try_get::<Option<i64>, _>("output_tokens")?.unwrap_or(0),
            };
            Ok(PurposeCalls {
                usd: metrics::priced(&model, &purpose, &counts, prices)?,
                calls: row.try_get("calls")?,
                purpose,
                model,
                counts,
            })
        })
        .collect()
}

/// The arm's summary tree nodes, written and free.
pub async fn tree_nodes(pool: &PgPool) -> Fallible<TreeNodes> {
    let row = sqlx::query(
        "SELECT count(*) FILTER (WHERE model IS NOT NULL)::int8 AS written, \
                count(*) FILTER (WHERE model IS NULL)::int8     AS free \
           FROM summary_tree_nodes",
    )
    .fetch_one(pool)
    .await?;
    Ok(TreeNodes {
        written: row.try_get("written")?,
        free: row.try_get("free")?,
    })
}

/// Every call outside the agent's own turns.
pub fn auxiliary(calls: &[PurposeCalls]) -> impl Iterator<Item = &PurposeCalls> {
    calls.iter().filter(|group| group.purpose != TURN_PURPOSE)
}
