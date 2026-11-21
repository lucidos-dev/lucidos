//! Tree backfill progress and estimates carried by system events.

use serde::{Deserialize, Serialize};

/// A low and a high figure.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bounds<T> {
    pub low: T,
    pub high: T,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EffortCost {
    pub effort: String,
    /// `TreeBackfillEstimate::usable_secs` at this tier's seconds per call.
    pub usable_secs: Bounds<u64>,
    /// `TreeBackfillEstimate::complete_secs` at this tier's seconds per call.
    pub complete_secs: Bounds<u64>,
    pub backfill_usd: Bounds<f64>,
    /// The single figure the panel leads with: the midpoint of the calls at
    /// the tier's central output. See `priced_work`.
    pub backfill_usd_central: f64,
    pub daily_usd: Bounds<f64>,
    pub daily_usd_central: f64,
    /// What one turn costs once the backfill is done, at the standard rate.
    pub turn_usd: Bounds<f64>,
    pub turn_usd_central: f64,
    /// Where a backfill at this tier pauses for the user: see
    /// `cap_for`.
    pub cap_usd: f64,
}

/// How far the backfill has got, in scopes: each in-scope thread plus the
/// workspace. A scope counts as done once its drain completes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BackfillProgress {
    pub done: usize,
    /// Zero until the compactor has seeded.
    pub total: usize,
    /// `done` in thousandths of a scope, plus the built share of each scope
    /// under way. One scope can hold hundreds of nodes, so this moves the bar
    /// within a long one. Only new work in a scope under way lowers it.
    pub done_milli: u64,
    /// Nodes built so far in the owed scopes under way: started draining and
    /// not done.
    pub nodes_done: u64,
    /// Nodes those scopes need in all, built or not.
    pub nodes_total: u64,
    /// The last drain failed because no background model could be resolved,
    /// so the backfill cannot move until one is configured.
    pub waiting_for_model: bool,
    /// An owed scope's last drain failed for another reason. It goes back
    /// after the retry interval, and this holds until it completes.
    pub retrying: bool,
    /// The model whose breaker is open: its calls keep failing, so nodes wait
    /// before the next one. Cleared when the model answers or the user picks
    /// another.
    #[serde(default)]
    pub failing_model: Option<String>,
    #[serde(default)]
    pub spend_cap_held_until: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub spend_cap_resets_at: Option<chrono::DateTime<chrono::Utc>>,
    /// The ready flag is set, so turns read the trees while the older
    /// threads still fill in.
    pub ready: bool,
    /// What the backfill episode has spent, when one is open.
    pub spent_usd: Option<f64>,
    /// Where it pauses, when its model is priced.
    pub cap_usd: Option<f64>,
    /// The cap Continue would set, when the backfill can have one.
    pub next_cap_usd: Option<f64>,
    /// The backfill spent its cap and waits for the user. Live work goes on.
    pub held: bool,
    /// The order of the frame that announced this count, rising across one
    /// engine's life. A snapshot carries the last frame's order, so a client
    /// drops a frame its snapshot already counted.
    pub seq: u64,
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
