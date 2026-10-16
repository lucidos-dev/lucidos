//! The *model price catalog*: what each model charges per million tokens, and
//! what its prompt cache charges as a share of the input price.
//!
//! No provider publishes prices in a machine-readable form, so the engine
//! carries them. Each row cites its source. The compactor's cost estimate and
//! the cache keep-alive's budget both read this one table, so no model is
//! priced twice (ADR 0489).

use super::served_model::served_as_sent;

/// What a cached input token costs, as a share of the input price.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CacheShares {
    pub(crate) read: f64,
    pub(crate) write_5m: f64,
    /// Equal to `write_5m` for a provider whose cache has one write rate.
    pub(crate) write_1h: f64,
}

/// One model's list price.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ModelPrice {
    pub(crate) id: &'static str,
    /// USD per million input tokens.
    pub(crate) input: f64,
    /// USD per million output tokens.
    pub(crate) output: f64,
    pub(crate) cache: CacheShares,
}

/// What a Claude model not in the catalog pays: the standard multipliers.
/// The lowest write-to-read ratio of any current Claude model, so an unknown
/// model gets the shortest keep-alive budget.
pub(crate) const STANDARD_CACHE: CacheShares = CacheShares {
    read: 0.1,
    write_5m: 1.25,
    write_1h: 2.0,
};

/// A Claude row, priced as the pricing page lists it:
/// <https://platform.claude.com/docs/en/about-claude/pricing>. Writes use the
/// standard multipliers. Only the read share differs between models.
const fn claude(id: &'static str, input: f64, output: f64, read: f64) -> ModelPrice {
    ModelPrice {
        id,
        input,
        output,
        cache: CacheShares {
            read,
            ..STANDARD_CACHE
        },
    }
}

/// The read share of Claude Fable 5.1 and Claude Mythos 5.1.
const READ_FABLE_5_1: f64 = 0.025;
/// The read share of Claude Opus 5.5 and Claude Sonnet 5.5.
const READ_5_5: f64 = 0.05;

pub(crate) const GPT_6_1_SOL: ModelPrice = ModelPrice {
    id: "gpt-6.1-sol",
    // https://developers.openai.com/api/docs/models/gpt-6.1-sol
    input: 2.0,
    output: 10.0,
    // https://developers.openai.com/api/docs/guides/prompt-caching
    cache: CacheShares {
        read: 0.05,
        write_5m: 1.25,
        write_1h: 1.25,
    },
};

pub(crate) const GEMINI_3_8_FLASH: ModelPrice = ModelPrice {
    id: "gemini-3.8-flash",
    // Introductory: it doubles to (1.5, 7.5) on 1 January 2027
    // (docs/temporary-measures.md, "Gemini 3.8 Flash's introductory
    // compactor price").
    input: 0.75,
    output: 3.75,
    // https://ai.google.dev/gemini-api/docs/pricing, "Context caching
    // price". Implicit caching writes at no premium.
    cache: CacheShares {
        read: 0.1,
        write_5m: 1.0,
        write_1h: 1.0,
    },
};

pub(crate) const CLAUDE_SONNET_5_5: ModelPrice = claude("claude-sonnet-5-5", 2.0, 10.0, READ_5_5);

/// Every priced model. Haiku 5.5 is its base tier, a prompt of up to 100,000
/// tokens. The higher tier keeps the same shares.
pub(crate) const CATALOG: &[ModelPrice] = &[
    GPT_6_1_SOL,
    GEMINI_3_8_FLASH,
    claude("claude-fable-5-1", 10.0, 50.0, READ_FABLE_5_1),
    claude("claude-mythos-5-1", 10.0, 50.0, READ_FABLE_5_1),
    claude("claude-fable-5", 10.0, 50.0, 0.1),
    claude("claude-mythos-5", 10.0, 50.0, 0.1),
    claude("claude-opus-5-5", 4.0, 20.0, READ_5_5),
    claude("claude-opus-5", 5.0, 25.0, 0.1),
    claude("claude-opus-4-8", 5.0, 25.0, 0.1),
    claude("claude-opus-4-7", 5.0, 25.0, 0.1),
    claude("claude-opus-4-6", 5.0, 25.0, 0.1),
    claude("claude-opus-4-5", 5.0, 25.0, 0.1),
    CLAUDE_SONNET_5_5,
    claude("claude-sonnet-5", 2.0, 10.0, 0.1),
    claude("claude-sonnet-4-6", 3.0, 15.0, 0.1),
    claude("claude-sonnet-4-5", 3.0, 15.0, 0.1),
    claude("claude-haiku-5-5", 0.1, 0.5, 0.1),
    claude("claude-haiku-4-5", 1.0, 5.0, 0.1),
];

/// The row for `model`, as a call or a capture names it. A `[1m]` suffix, a
/// `vendor/` prefix and a dated snapshot find the base model's row.
pub(crate) fn price_of(model: &str) -> Option<&'static ModelPrice> {
    CATALOG.iter().find(|row| served_as_sent(row.id, model))
}

/// What `model`'s prompt cache charges: its row, else the standard shares.
pub(crate) fn cache_shares_of(model: &str) -> CacheShares {
    price_of(model).map_or(STANDARD_CACHE, |row| row.cache)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_id_is_unique() {
        let mut ids: Vec<&str> = CATALOG.iter().map(|row| row.id).collect();
        let before = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), before, "a duplicate id in CATALOG");
    }

    /// A model priced twice is the failure the catalog exists to prevent.
    #[test]
    fn no_id_finds_two_rows() {
        for row in CATALOG {
            let found = CATALOG
                .iter()
                .filter(|other| served_as_sent(other.id, row.id))
                .count();
            assert_eq!(found, 1, "{} matches {found} rows", row.id);
        }
    }

    /// `[1m]`, `vendor/` and snapshot ids price as their base model, and a
    /// later minor version does not price as its neighbour.
    #[test]
    fn recorded_model_ids_find_their_row() {
        let found = |id: &str| price_of(id).map(|row| row.id);
        for id in [
            "claude-opus-5-5",
            "claude-opus-5-5[1m]",
            "anthropic/claude-opus-5-5",
            "claude-opus-5-5@20260901",
            "claude-opus-5-5-20260901",
        ] {
            assert_eq!(found(id), Some("claude-opus-5-5"), "{id}");
        }
        assert_eq!(found("claude-opus-5"), Some("claude-opus-5"));
        assert_eq!(found("claude-fable-5-1"), Some("claude-fable-5-1"));
        assert_eq!(found("claude-fable-5"), Some("claude-fable-5"));
        assert_eq!(found("my-local-model"), None);
    }

    #[test]
    fn an_unknown_model_pays_the_standard_shares() {
        assert_eq!(cache_shares_of("my-local-model"), STANDARD_CACHE);
        assert_eq!(cache_shares_of("claude-opus-5-5[1m]").read, READ_5_5);
    }

    /// The fallback is the shortest budget, so no Claude row may undercut
    /// its write-to-read ratio at either TTL. Other providers' caches write
    /// at other ratios, and the keep-alive never prices them.
    #[test]
    fn no_claude_row_undercuts_the_fallback() {
        let ratios = |c: CacheShares| (c.write_5m / c.read, c.write_1h / c.read);
        let floor = ratios(STANDARD_CACHE);
        for row in CATALOG.iter().filter(|row| row.id.starts_with("claude-")) {
            let (five, hour) = ratios(row.cache);
            assert!(five >= floor.0 && hour >= floor.1, "{}", row.id);
        }
    }

    /// The pricing page's ratios, which the keep-alive budget follows.
    #[test]
    fn claude_ratios_match_the_pricing_page() {
        let ratios = |id: &str| {
            let cache = price_of(id).expect("priced").cache;
            (cache.write_5m / cache.read, cache.write_1h / cache.read)
        };
        for (id, (five, hour)) in [
            ("claude-fable-5-1", (50.0, 80.0)),
            ("claude-opus-5-5", (25.0, 40.0)),
            ("claude-sonnet-5-5", (25.0, 40.0)),
            ("claude-opus-5", (12.5, 20.0)),
            ("claude-haiku-4-5", (12.5, 20.0)),
        ] {
            let (got_five, got_hour) = ratios(id);
            assert!((got_five - five).abs() < 1e-9, "{id}: {got_five}");
            assert!((got_hour - hour).abs() < 1e-9, "{id}: {got_hour}");
        }
    }
}
