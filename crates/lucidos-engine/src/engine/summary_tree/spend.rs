//! What a Tree backfill has spent, against the cap it pauses at.
//!
//! **A backfill episode** runs from `TreeBackfillStarted` to
//! `TreeBackfillEnded` or `TreeBackfillReset`. Its spend is every compactor
//! call recorded since it started, priced by [`COMPACTOR_MODELS`]. Its cap is
//! the one the newest `TreeBackfillStarted` or `TreeBackfillCapRaised` names.
//!
//! **Events are the authority.** The compactor keeps a running total between
//! seeds, and each seed reads the episode again from the events. So a restart
//! holds a backfill that had reached its cap.
//!
//! [`COMPACTOR_MODELS`]: super::compactor_models::COMPACTOR_MODELS

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use super::compactor_models::{priced, Tokens};
use super::estimate::{compaction_captures_where, Bounds, EffortCost};
use crate::llm::ServiceTier;

/// The cap, as a multiple of the estimate's central figure.
const CAP_MULTIPLE: f64 = 1.5;

/// How far one raise moves the cap, as a share of the central figure.
const RAISE_SHARE: f64 = 0.5;

/// The cap a backfill estimated at `central` dollars pauses at.
pub(crate) fn cap_for(central: f64) -> f64 {
    central * CAP_MULTIPLE
}

/// What a backfill from here would cost on the compactor's model, as the
/// Start dialog shows it. `TreeBackfillStarted` carries it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StartEstimate {
    pub model: String,
    pub effort: Option<String>,
    pub calls: Bounds<u64>,
    /// `None` for a model the catalog does not price, which gets no cap.
    pub cost: Option<EffortCost>,
}

/// The open episode, as the events record it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Episode {
    /// The estimate's central figure, `None` for an unpriced model.
    pub(crate) central_usd: Option<f64>,
    pub(crate) cap_usd: Option<f64>,
    pub(crate) spent_usd: f64,
    /// A `TreeBackfillCapReached` already names the current cap.
    pub(crate) cap_announced: bool,
}

impl Episode {
    /// What the episode has spent, when its model is priced. An unpriced
    /// model's calls add nothing, so a spend of zero would mislead.
    pub(crate) fn priced_spend(&self) -> Option<f64> {
        self.central_usd.map(|_| self.spent_usd)
    }

    /// The backfill waits for the user: it has spent its cap.
    pub(crate) fn held(&self) -> bool {
        self.cap_usd.is_some_and(|cap| self.spent_usd >= cap)
    }

    /// The cap one press of Continue sets: half the estimate past whichever
    /// is higher, the cap or the spend. In-flight calls finish after a hold,
    /// so the spend can pass the cap a little.
    pub(crate) fn next_cap(&self) -> Option<f64> {
        Some(self.cap_usd?.max(self.spent_usd) + self.central_usd? * RAISE_SHARE)
    }

    /// Price `tokens` of `model`, served on `tier`, into the spend. `true`
    /// when this call reached the cap and nothing has announced it yet.
    pub(crate) fn spend(&mut self, model: &str, tier: Option<ServiceTier>, tokens: Tokens) -> bool {
        let Some(priced) = priced(model) else {
            return false;
        };
        self.spent_usd += priced.usd(tokens, tier);
        let reached = self.held() && !self.cap_announced;
        self.cap_announced |= reached;
        reached
    }
}

/// The newest `TreeBackfillStarted`, and whether a later row closed it.
const NEWEST_START_SQL: &str = "SELECT s.sequence, s.payload->'data', EXISTS ( \
       SELECT 1 FROM events e \
       WHERE e.event_type IN ('TreeBackfillEnded', 'TreeBackfillReset') \
         AND e.sequence > s.sequence) \
     FROM events s WHERE s.event_type = 'TreeBackfillStarted' \
     ORDER BY s.sequence DESC LIMIT 1";

/// The newest raise after the start in `$1`.
const NEWEST_RAISE_SQL: &str = "SELECT sequence, (payload->'data'->>'cap_usd')::float8 \
     FROM events WHERE event_type = 'TreeBackfillCapRaised' AND sequence > $1 \
     ORDER BY sequence DESC LIMIT 1";

const CAP_REACHED_SINCE_SQL: &str = "SELECT EXISTS (SELECT 1 FROM events \
     WHERE event_type = 'TreeBackfillCapReached' AND sequence > $1)";

/// Compactor tokens per model and service tier since the sequence in `$1`.
const SPENT_SQL: &str = concat!(
    "SELECT payload->>'model', payload->'usage'->>'service_tier', \
       sum((payload->'usage'->>'input_tokens')::bigint)::float8, \
       sum(COALESCE((payload->'usage'->>'cache_read_tokens')::bigint, 0))::float8, \
       sum(COALESCE((payload->'usage'->>'cache_creation_tokens')::bigint, 0))::float8, \
       sum((payload->'usage'->>'output_tokens')::bigint)::float8 \
     FROM events WHERE ",
    compaction_captures_where!(),
    " AND payload ? 'usage' AND sequence > $1 GROUP BY 1, 2"
);

/// The open episode, or `None` when no backfill episode is open.
pub(crate) async fn load(pool: &PgPool) -> Result<Option<Episode>, sqlx::Error> {
    let start: Option<(i64, serde_json::Value, bool)> = sqlx::query_as(NEWEST_START_SQL)
        .fetch_optional(pool)
        .await?;
    let Some((started_seq, data, false)) = start else {
        return Ok(None);
    };
    let raise: Option<(i64, Option<f64>)> = sqlx::query_as(NEWEST_RAISE_SQL)
        .bind(started_seq)
        .fetch_optional(pool)
        .await?;
    let (cap_seq, cap_usd) = match raise {
        Some((seq, cap)) => (seq, cap),
        None => (started_seq, data["cap_usd"].as_f64()),
    };
    let cap_announced: bool = sqlx::query_scalar(CAP_REACHED_SINCE_SQL)
        .bind(cap_seq)
        .fetch_one(pool)
        .await?;
    Ok(Some(Episode {
        central_usd: data["estimate"]["cost"]["backfill_usd_central"].as_f64(),
        cap_usd,
        spent_usd: spent_since(pool, started_seq).await?,
        cap_announced,
    }))
}

/// What the compactor has spent since `seq`. A model the catalog does not
/// price adds nothing, since nothing could cap it.
pub(crate) async fn spent_since(pool: &PgPool, seq: i64) -> Result<f64, sqlx::Error> {
    type Row = (
        Option<String>,
        Option<String>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
    );
    let rows: Vec<Row> = sqlx::query_as(SPENT_SQL).bind(seq).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .filter_map(|(model, tier, input, cache_read, cache_write, output)| {
            let tokens = Tokens {
                input: input.unwrap_or_default(),
                cache_read: cache_read.unwrap_or_default(),
                cache_write: cache_write.unwrap_or_default(),
                output: output.unwrap_or_default(),
            };
            let tier = tier.as_deref().and_then(ServiceTier::from_wire);
            Some(priced(&model?)?.usd(tokens, tier))
        })
        .sum())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode(cap: Option<f64>, spent: f64) -> Episode {
        Episode {
            central_usd: Some(10.0),
            cap_usd: cap,
            spent_usd: spent,
            cap_announced: false,
        }
    }

    /// The prose that states the cap and the raise reads these constants.
    /// A pin, since prose cannot import them.
    #[test]
    fn every_prose_copy_states_the_cap_and_the_raise() {
        let doc = |path: &str| {
            std::fs::read_to_string(format!("{}/../../{path}", env!("CARGO_MANIFEST_DIR")))
                .unwrap_or_else(|e| panic!("{path}: {e}"))
        };
        let cap = format!("{CAP_MULTIPLE} times");
        let raise = match RAISE_SHARE {
            0.5 => "half the estimate",
            other => panic!("say how far a raise moves the cap ({other}), then pin it here"),
        };
        for path in ["system-knowhow/preferences.md", "docs/glossary.md"] {
            assert!(doc(path).contains(&cap), "{path} omits {cap}");
        }
        for path in ["system-knowhow/thread-events.md", "docs/glossary.md"] {
            assert!(doc(path).contains(raise), "{path} omits {raise}");
        }
    }

    #[test]
    fn an_episode_holds_once_it_has_spent_its_cap() {
        assert!(!episode(Some(15.0), 14.99).held());
        assert!(episode(Some(15.0), 15.0).held());
        assert!(!episode(None, 1e9).held(), "no cap never holds");
    }

    /// A raise moves past the spend when in-flight calls overshot the cap.
    #[test]
    fn a_raise_adds_a_share_of_the_estimate_past_the_cap_or_the_spend() {
        let step = 10.0 * RAISE_SHARE;
        assert_eq!(episode(Some(15.0), 15.2).next_cap(), Some(15.2 + step));
        assert_eq!(episode(Some(15.0), 3.0).next_cap(), Some(15.0 + step));
        assert_eq!(episode(None, 3.0).next_cap(), None);
    }

    /// Only the call that crosses the cap reports it, once.
    #[test]
    fn the_crossing_call_reports_the_cap_once() {
        let call = Tokens {
            input: 100.0,
            output: 10.0,
            ..Tokens::default()
        };
        let one = priced("gpt-6.1-sol").expect("priced").usd(call, None);
        let mut e = episode(Some(one * 1.5), 0.0);
        assert!(
            !e.spend("gpt-6.1-sol", None, call),
            "one call is under the cap"
        );
        assert!(e.spend("gpt-6.1-sol", None, call), "two cross it");
        assert!(!e.spend("gpt-6.1-sol", None, call), "already announced");
        assert!(!e.spend("an-unpriced-model", None, call));
        assert!((e.spent_usd - 3.0 * one).abs() < 1e-12);
    }

    /// A Flex call costs Sol its Flex share of every rate, cache reads and
    /// writes included.
    #[test]
    fn a_flex_call_is_priced_at_the_flex_rate() {
        let call = Tokens {
            input: 1_000.0,
            cache_read: 500.0,
            cache_write: 300.0,
            output: 100.0,
        };
        let sol = priced("gpt-6.1-sol").expect("Sol is priced");
        let share = sol.flex.expect("Sol has a Flex rate");
        let standard = sol.usd(call, None);
        assert!((sol.usd(call, Some(ServiceTier::Flex)) - standard * share).abs() < 1e-12);
        let flash = priced("gemini-3.8-flash").expect("Flash is priced");
        assert_eq!(
            flash.usd(call, Some(ServiceTier::Flex)),
            flash.usd(call, None),
            "no Flex rate prices at the standard rate"
        );
    }
}
