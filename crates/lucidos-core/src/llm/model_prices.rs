use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::SystemTime;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use super::served_model::served_as_sent;

pub const OVERRIDE_PATH: &str = "data/config/model-prices.json";

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rates {
    pub uncached_in: f64,
    pub cache_write: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_1h: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_12h: Option<f64>,
    pub cache_read: f64,
    pub out: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LongRates {
    pub threshold_tokens: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub inclusive: bool,
    #[serde(flatten)]
    pub rates: Rates,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PriceCard {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<NaiveDate>,
    #[serde(flatten)]
    pub rates: Rates,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long: Option<LongRates>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flex: Option<Rates>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_minute: Option<f64>,
    pub source: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CardOrigin {
    Shipped,
    Override,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TableCard {
    #[serde(flatten)]
    pub card: PriceCard,
    pub origin: CardOrigin,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CacheShares {
    pub read: f64,
    pub write_5m: f64,
    pub write_1h: f64,
    pub write_12h: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct OverrideState {
    pub path: &'static str,
    pub present: bool,
    pub checked: Option<NaiveDate>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PriceTable {
    pub currency: String,
    pub checked: NaiveDate,
    /// INVARIANT: What an unpriced model's cache costs: the standard Claude multipliers,
    /// the lowest write-to-read ratio of any Claude model, so an unknown
    /// model gets the shortest keep-alive budget.
    pub unpriced_cache_shares: CacheShares,
    /// INVARIANT: Each model's cards, oldest first.
    pub models: BTreeMap<String, Vec<TableCard>>,
    #[serde(rename = "override")]
    pub override_state: OverrideState,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelPrice {
    pub input: f64,
    pub output: f64,
    pub cache: CacheShares,
    pub flex: Option<f64>,
    pub long: Option<LongTier>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LongTier {
    pub threshold_tokens: u64,
    pub factor: f64,
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

pub fn shipped_table() -> Arc<PriceTable> {
    SHIPPED.clone()
}

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

    pub fn card_on(&self, model: &str, day: NaiveDate) -> Option<&PriceCard> {
        self.cards(model)?
            .iter()
            .rev()
            .map(|c| &c.card)
            .find(|card| card.from.is_none_or(|from| from <= day))
    }

    pub fn price_on(&self, model: &str, day: NaiveDate) -> Option<ModelPrice> {
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

pub fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

static OVERRIDE_WORKSPACE: OnceLock<PathBuf> = OnceLock::new();

pub fn read_overrides_from(workspace: PathBuf) {
    if OVERRIDE_WORKSPACE.set(workspace).is_err() {
        crate::log_legacy!("[ModelPrices] override workspace already set; keeping the first");
    }
}

pub fn price_table() -> Arc<PriceTable> {
    OVERRIDE_WORKSPACE
        .get()
        .map_or_else(shipped_table, |ws| price_table_for(ws))
}

type FileStamp = (SystemTime, u64);

struct Loaded {
    stamp: Option<FileStamp>,
    table: Arc<PriceTable>,
}

static LOADED: LazyLock<Mutex<HashMap<PathBuf, Loaded>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

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
        crate::log_legacy!("[ModelPrices] ignoring {}: {error}", path.display());
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

pub fn price_of(model: &str) -> Option<ModelPrice> {
    price_table().price_on(model, today())
}

pub fn cache_shares_of(model: &str) -> CacheShares {
    let table = price_table();
    table
        .price_on(model, today())
        .map_or(table.unpriced_cache_shares, |price| price.cache)
}

#[cfg(test)]
#[path = "model_prices_tests.rs"]
mod tests;
