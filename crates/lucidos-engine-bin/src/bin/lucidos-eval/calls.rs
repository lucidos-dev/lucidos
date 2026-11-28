use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};

use crate::config::ModelPrice;
use crate::metrics::{self, TokenCounts};

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub const TURN_PURPOSE: &str = "turn";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PurposeCalls {
    pub purpose: String,
    pub model: String,
    pub calls: i64,
    pub counts: TokenCounts,
    pub usd: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeNodes {
    pub written: i64,
    pub free: i64,
}

pub async fn by_purpose(pool: &PgPool, prices: &[ModelPrice]) -> Fallible<Vec<PurposeCalls>> {
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

pub fn auxiliary(calls: &[PurposeCalls]) -> impl Iterator<Item = &PurposeCalls> {
    calls.iter().filter(|group| group.purpose != TURN_PURPOSE)
}
