//! The *price table*: what each model charges per million tokens, card by
//! dated card (ADR 0525).
//!
//! The shipped cards live in `model_prices.json`. A workspace overrides them in
//! [`OVERRIDE_PATH`], merged card by card and re-read when the file changes.
//! Every reader prices through here: the compactor's estimate, the cache
//! keep-alive, the handoff estimate, gateway telemetry and the intelligence
//! ladder. No provider publishes prices in a machine-readable form, so each
//! card cites its source (ADR 0489).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::SystemTime;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use super::served_model::served_as_sent;

/// The workspace override, relative to the workspace directory.
pub const OVERRIDE_PATH: &str = "data/config/model-prices.json";

/// One tier's rates, in the table's currency per million tokens.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rates {
    pub uncached_in: f64,
    /// The 5-minute cache write.
    pub cache_write: f64,
    /// The 1-hour cache write. Absent means `cache_write`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_1h: Option<f64>,
    /// OpenAI's 12-hour cache write. Absent means `cache_write`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_12h: Option<f64>,
    pub cache_read: f64,
    pub out: f64,
}

/// The long-context tier: its rates replace the base ones for every token of
/// a call whose prompt reached `threshold_tokens`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LongRates {
    pub threshold_tokens: u64,
    /// The tier starts at the threshold itself rather than past it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub inclusive: bool,
    #[serde(flatten)]
    pub rates: Rates,
}

/// One dated price of one model, in Token Cost's card format.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PriceCard {
    /// The first day the card is in force, local time. Absent means since the
    /// beginning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<NaiveDate>,
    #[serde(flatten)]
    pub rates: Rates,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long: Option<LongRates>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flex: Option<Rates>,
    /// A model billed by wall-clock minute. Its token rates are then unused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_minute: Option<f64>,
    pub source: String,
}

/// Which file a card came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CardOrigin {
    Shipped,
    Override,
}

/// A card of the merged table, with its provenance.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TableCard {
    #[serde(flatten)]
    pub card: PriceCard,
    pub origin: CardOrigin,
}

/// What a cached input token costs, as a share of the input price.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CacheShares {
    pub read: f64,
    pub write_5m: f64,
    /// Equal to `write_5m` for a provider whose cache has one write rate.
    pub write_1h: f64,
    /// Equal to `write_5m` for a provider with no 12-hour cache.
    pub write_12h: f64,
}

/// The state of the workspace override, as the merged table reports it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct OverrideState {
    pub path: &'static str,
    pub present: bool,
    /// The day the override's prices were last checked, as it records.
    pub checked: Option<NaiveDate>,
    /// Why the override was ignored. The shipped cards then stand alone.
    pub error: Option<String>,
}

/// The shipped cards merged with a workspace's override.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PriceTable {
    pub currency: String,
    /// The day the prices in force were last checked: the override's day
    /// when it is newer than the shipped one.
    pub checked: NaiveDate,
    /// What an unpriced model's cache costs: the standard Claude multipliers,
    /// the lowest write-to-read ratio of any Claude model, so an unknown
    /// model gets the shortest keep-alive budget.
    pub unpriced_cache_shares: CacheShares,
    /// Each model's cards, oldest first.
    pub models: BTreeMap<String, Vec<TableCard>>,
    #[serde(rename = "override")]
    pub override_state: OverrideState,
}

/// One model's price on one day, as the engine's estimates read it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ModelPrice {
    /// Per million input tokens.
    pub(crate) input: f64,
    /// Per million output tokens.
    pub(crate) output: f64,
    pub(crate) cache: CacheShares,
    /// The Flex tier's input rate as a share of the standard one. `None`
    /// without a Flex tier.
    pub(crate) flex: Option<f64>,
    pub(crate) long: Option<LongTier>,
}

/// A model's long-context tier, as a multiple of its base price.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LongTier {
    pub(crate) threshold_tokens: u64,
    /// The long input rate over the base input rate.
    pub(crate) factor: f64,
}

#[derive(Deserialize)]
struct ShippedFile {
    checked: NaiveDate,
    currency: String,
    unpriced_cache_shares: CacheShares,
    models: BTreeMap<String, Vec<PriceCard>>,
}

fn shipped_file() -> ShippedFile {
    serde_json::from_str(include_str!("model_prices.json"))
        .expect("llm/model_prices.json is malformed")
}

#[derive(Deserialize)]
struct OverrideFile {
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    checked: Option<NaiveDate>,
    #[serde(default)]
    models: BTreeMap<String, Vec<PriceCard>>,
}

static SHIPPED: LazyLock<Arc<PriceTable>> = LazyLock::new(|| Arc::new(merge(shipped_file(), None)));

/// The shipped cards alone, as a workspace with no override sees them.
pub fn shipped_table() -> Arc<PriceTable> {
    SHIPPED.clone()
}

/// Merge `overlay` over `shipped`, card by card. An override card replaces
/// the shipped card from the same day, and joins the list otherwise. So a
/// past day keeps the card that was in force on it. An invalid overlay is
/// ignored whole, and the error says why.
fn merge(shipped: ShippedFile, overlay: Option<Result<OverrideFile, String>>) -> PriceTable {
    let as_table = |cards: Vec<PriceCard>, origin| {
        cards
            .into_iter()
            .map(|card| TableCard { card, origin })
            .collect::<Vec<_>>()
    };
    let mut models: BTreeMap<String, Vec<TableCard>> = shipped
        .models
        .into_iter()
        .map(|(id, cards)| (id, as_table(cards, CardOrigin::Shipped)))
        .collect();
    let mut override_state = OverrideState {
        path: OVERRIDE_PATH,
        ..OverrideState::default()
    };
    if let Some(parsed) = overlay {
        override_state.present = true;
        match parsed.and_then(|file| validate_override(file, &shipped.currency)) {
            Err(error) => override_state.error = Some(error),
            Ok(file) => {
                override_state.checked = file.checked;
                for (id, cards) in file.models {
                    let list = models.entry(id).or_default();
                    for card in as_table(cards, CardOrigin::Override) {
                        list.retain(|old| old.card.from != card.card.from);
                        list.push(card);
                    }
                }
            }
        }
    }
    for list in models.values_mut() {
        list.sort_by_key(|c| c.card.from);
    }
    PriceTable {
        checked: override_state
            .checked
            .map_or(shipped.checked, |day| day.max(shipped.checked)),
        currency: shipped.currency,
        unpriced_cache_shares: shipped.unpriced_cache_shares,
        models,
        override_state,
    }
}

/// Refuse an override the merge would misread: another currency, two cards
/// for one model on one day, or a negative rate.
fn validate_override(file: OverrideFile, currency: &str) -> Result<OverrideFile, String> {
    if let Some(theirs) = file.currency.as_deref().filter(|c| *c != currency) {
        return Err(format!(
            "its currency is {theirs}, and the shipped prices are in {currency}"
        ));
    }
    let negative = |r: &Rates| {
        [r.uncached_in, r.cache_write, r.cache_read, r.out]
            .into_iter()
            .chain(r.cache_write_1h)
            .chain(r.cache_write_12h)
            .any(|v| v < 0.0)
    };
    for (id, cards) in &file.models {
        let mut days: Vec<_> = cards.iter().map(|c| c.from).collect();
        days.sort_unstable();
        if days.windows(2).any(|w| w[0] == w[1]) {
            return Err(format!("{id} has two cards from the same day"));
        }
        let any_negative = cards.iter().any(|c| {
            negative(&c.rates)
                || c.long.is_some_and(|l| negative(&l.rates))
                || c.flex.as_ref().is_some_and(negative)
        });
        if any_negative {
            return Err(format!("{id} has a negative rate"));
        }
    }
    Ok(file)
}

impl PriceTable {
    /// `model`'s cards: its own row, else the row it is served as. A `[1m]`
    /// suffix, a `vendor/` prefix and a dated snapshot find the base row.
    fn cards(&self, model: &str) -> Option<&[TableCard]> {
        self.models
            .get(model)
            .or_else(|| {
                self.models
                    .iter()
                    .find(|(id, _)| served_as_sent(id, model))
                    .map(|(_, cards)| cards)
            })
            .map(Vec::as_slice)
    }

    /// The card in force for `model` on `day`: the newest one from that day
    /// or before.
    pub fn card_on(&self, model: &str, day: NaiveDate) -> Option<&PriceCard> {
        self.cards(model)?
            .iter()
            .rev()
            .map(|c| &c.card)
            .find(|card| card.from.is_none_or(|from| from <= day))
    }

    pub(crate) fn price_on(&self, model: &str, day: NaiveDate) -> Option<ModelPrice> {
        self.card_on(model, day)
            .map(|card| ModelPrice::of(card, self.unpriced_cache_shares))
    }
}

impl ModelPrice {
    fn of(card: &PriceCard, unpriced: CacheShares) -> Self {
        let rates = card.rates;
        let input = rates.uncached_in;
        let share = |rate: f64| (input > 0.0).then(|| rate / input);
        let cache = match (share(rates.cache_read), share(rates.cache_write)) {
            (Some(read), Some(write_5m)) => CacheShares {
                read,
                write_5m,
                write_1h: share(rates.cache_write_1h.unwrap_or(rates.cache_write))
                    .unwrap_or(write_5m),
                write_12h: share(rates.cache_write_12h.unwrap_or(rates.cache_write))
                    .unwrap_or(write_5m),
            },
            _ => unpriced,
        };
        Self {
            input,
            output: rates.out,
            cache,
            flex: card.flex.and_then(|flex| share(flex.uncached_in)),
            long: card.long.and_then(|long| {
                share(long.rates.uncached_in).map(|factor| LongTier {
                    threshold_tokens: long.threshold_tokens,
                    factor,
                })
            }),
        }
    }
}

/// The day a price applies to now, local time, as a card's `from` reads.
pub(crate) fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

/// The workspace whose override the engine's own readers merge. Set once at
/// boot. Unset, they read the shipped cards alone.
static OVERRIDE_WORKSPACE: OnceLock<PathBuf> = OnceLock::new();

/// Merge `workspace`'s override into every price the engine reads from now on.
pub fn read_overrides_from(workspace: PathBuf) {
    if OVERRIDE_WORKSPACE.set(workspace).is_err() {
        crate::log!("[ModelPrices] override workspace already set; keeping the first");
    }
}

/// The price table the engine's readers use.
pub fn price_table() -> Arc<PriceTable> {
    OVERRIDE_WORKSPACE
        .get()
        .map_or_else(shipped_table, |ws| price_table_for(ws))
}

/// The override file's identity: a changed stamp means re-read it.
type FileStamp = (SystemTime, u64);

struct Loaded {
    stamp: Option<FileStamp>,
    table: Arc<PriceTable>,
}

static LOADED: LazyLock<Mutex<HashMap<PathBuf, Loaded>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The price table merged with `workspace`'s override, re-read when the file
/// changes.
pub fn price_table_for(workspace: &Path) -> Arc<PriceTable> {
    let path = workspace.join(OVERRIDE_PATH);
    let stamp = std::fs::metadata(&path)
        .ok()
        .map(|m| (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len()));
    let mut loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(cached) = loaded.get(&path).filter(|l| l.stamp == stamp) {
        return cached.table.clone();
    }
    let overlay = stamp.is_some().then(|| {
        std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|text| serde_json::from_str(&text).map_err(|e| e.to_string()))
    });
    let table = Arc::new(merge(shipped_file(), overlay));
    if let Some(error) = &table.override_state.error {
        crate::log!("[ModelPrices] ignoring {}: {error}", path.display());
    }
    loaded.insert(
        path,
        Loaded {
            stamp,
            table: table.clone(),
        },
    );
    table
}

/// `model`'s price today, from the engine's price table.
pub(crate) fn price_of(model: &str) -> Option<ModelPrice> {
    price_table().price_on(model, today())
}

/// What `model`'s prompt cache charges today: its card, else the unpriced
/// shares.
pub(crate) fn cache_shares_of(model: &str) -> CacheShares {
    let table = price_table();
    table
        .price_on(model, today())
        .map_or(table.unpriced_cache_shares, |price| price.cache)
}

#[cfg(test)]
#[path = "model_prices_tests.rs"]
mod tests;
