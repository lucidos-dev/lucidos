//! What one API call reported it used, and what that cost.

use serde::{Deserialize, Serialize};

/// Tokens one API call reported.
///
/// **`input_tokens` is a TOTAL and it CONTAINS the two cache counts.** It is
/// everything the model processed, which is the number a reader wants for
/// context size. Both providers are normalised to that convention: Anthropic
/// reports three disjoint counts and `anthropic_wire` sums them, and OpenAI's
/// `prompt_tokens` already covers `prompt_tokens_details.cached_tokens`.
///
/// So the three input fields are a total and two of its parts, never three
/// classes to add up. Anything applying a per-class rate must first derive
/// fresh input as `input_tokens - cache_read_tokens - cache_creation_tokens`,
/// saturating. Read as classes, every cached token is billed twice, once at
/// the full input rate. That mistake overstated the eval's dollar figures
/// fourfold, and `lucidos-eval`'s `InputSplit` is the type that now prevents it.
///
/// `cache_read_tokens` is set by both providers. `cache_creation_tokens` is
/// set by Anthropic, and by OpenAI from GPT-5.6 on. `output_tokens` may be zero on a snapshot emitted mid-stream, before
/// the final delta.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiUsage {
    /// The TOTAL, and it CONTAINS the two cache counts below. Derive fresh
    /// input as `input_tokens - cache_read_tokens - cache_creation_tokens`,
    /// saturating. Read as three classes, every cached token counts twice.
    pub input_tokens: u32,
    /// May be zero on a snapshot emitted mid-stream, before the final delta.
    pub output_tokens: u32,
    /// Set by both providers.
    pub cache_read_tokens: u32,
    /// Anthropic, and OpenAI from GPT-5.6 on. Both bill a write above input.
    pub cache_creation_tokens: u32,
    /// How the four counts above split across text, audio and image. Only a
    /// realtime voice call reports one, so it is absent everywhere else and
    /// every other producer writes the payload it always wrote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modality: Option<ModalityUsage>,
    /// The tier that served the call, when not the provider's standard one.
    /// Its rates differ, so a price reads it beside the four counts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<crate::llm::ServiceTier>,
    /// Part of `cache_creation_tokens`: the tokens written to the 1-hour
    /// cache, billed at 2x base input rather than 1.25x. A Claude Code
    /// session and a Claude chat call on Anthropic or Vertex split a write by
    /// lifetime. The rest of the write went to the 5-minute cache.
    /// `CacheTtl::of_write` reads the call's TTL from the split.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_1h_tokens: Option<u32>,
    /// Part of `cache_creation_tokens`: the tokens OpenAI's Responses API
    /// wrote to its 30-minute cache. With the 12-hour part it covers the write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_30m_tokens: Option<u32>,
    /// Part of `cache_creation_tokens`: the tokens OpenAI's Responses API
    /// wrote to its 12-hour cache, billed above the 30-minute write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_12h_tokens: Option<u32>,
    /// The cache lifetime the request asked for, where the response split
    /// nothing. The router sets it only for a route probes showed honours it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_requested_ttl: Option<crate::llm::cache_lifetime::CacheLifetime>,
}

impl ApiUsage {
    /// The prompt tokens neither read from nor written to the cache.
    pub(crate) fn uncached_input_tokens(&self) -> u32 {
        self.input_tokens
            .saturating_sub(self.cache_read_tokens)
            .saturating_sub(self.cache_creation_tokens)
    }

    /// What this call cost at `model`'s list price on `day`, in USD. `None`
    /// for an unpriced model. A prompt that reached the card's long tier
    /// prices every token at its rates. A Flex call prices as standard.
    pub(crate) fn usd_on(&self, model: &str, day: chrono::NaiveDate) -> Option<f64> {
        let table = crate::llm::model_prices::price_table();
        let card = table.card_on(model, day)?;
        let prompt = u64::from(self.input_tokens);
        let rates = match card.long {
            Some(long)
                if prompt > long.threshold_tokens
                    || (long.inclusive && prompt == long.threshold_tokens) =>
            {
                long.rates
            }
            _ => card.rates,
        };
        let read = f64::from(self.cache_read_tokens);
        let written = f64::from(self.cache_creation_tokens);
        let written_1h = f64::from(self.cache_creation_1h_tokens.unwrap_or(0)).min(written);
        let written_12h =
            f64::from(self.cache_creation_12h_tokens.unwrap_or(0)).min(written - written_1h);
        let fresh = (f64::from(self.input_tokens) - read - written).max(0.0);
        let input_usd = fresh * rates.uncached_in
            + read * rates.cache_read
            + (written - written_1h - written_12h) * rates.cache_write
            + written_1h * rates.cache_write_1h.unwrap_or(rates.cache_write)
            + written_12h * rates.cache_write_12h.unwrap_or(rates.cache_write);
        Some((input_usd + f64::from(self.output_tokens) * rates.out) / 1_000_000.0)
    }
}

/// How one call's tokens split across text, audio and image.
///
/// The realtime API bills per modality at rates an order of magnitude apart, so
/// a blended total cannot be priced. Every field is a PART of a count on
/// [`ApiUsage`], named after the count it belongs to. Three sums hold:
///
/// - input text + audio + image == `input_tokens`
/// - cache-read text + audio + image == `cache_read_tokens`
/// - output text + audio == `output_tokens`
///
/// **If a provider ever disagrees, the flat totals win and the parts are stored
/// exactly as reported.** Rescaling would invent numbers no frame carried, and
/// it would hide the drift from whoever has to fix it. The producer logs the
/// mismatch instead.
///
/// A breakdown is all-or-nothing: a producer that cannot fill every field
/// leaves `modality` as `None`. Zeros would read as a real turn that spoke
/// nothing, and the consumer could not tell the two apart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModalityUsage {
    /// Part of `input_tokens`, with the audio and image counts below.
    pub input_text_tokens: u32,
    /// Part of `input_tokens`.
    pub input_audio_tokens: u32,
    /// Part of `input_tokens`.
    pub input_image_tokens: u32,
    /// Part of `cache_read_tokens`, with the audio and image counts below.
    pub cache_read_text_tokens: u32,
    /// Part of `cache_read_tokens`.
    pub cache_read_audio_tokens: u32,
    /// Part of `cache_read_tokens`.
    pub cache_read_image_tokens: u32,
    /// Part of `output_tokens`, with the audio count below.
    pub output_text_tokens: u32,
    /// Part of `output_tokens`.
    pub output_audio_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u32, output: u32, read: u32, written: u32, written_1h: u32) -> ApiUsage {
        ApiUsage {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: read,
            cache_creation_tokens: written,
            cache_creation_1h_tokens: Some(written_1h),
            ..Default::default()
        }
    }

    /// OpenAI's 12-hour write bills at the card's 12-hour rate, and the
    /// 30-minute part at the plain write rate.
    #[test]
    fn a_12h_write_prices_at_its_own_rate() {
        let today = crate::llm::model_prices::today();
        let table = crate::llm::model_prices::price_table();
        let rates = table.card_on("gpt-6.1-sol", today).expect("priced").rates;
        let write_12h = rates
            .cache_write_12h
            .expect("a GPT-5.6+ card prices 12 hours");
        let call = ApiUsage {
            input_tokens: 1_300,
            cache_creation_tokens: 1_200,
            cache_creation_30m_tokens: Some(1_000),
            cache_creation_12h_tokens: Some(200),
            ..Default::default()
        };
        let expected =
            (100.0 * rates.uncached_in + 1_000.0 * rates.cache_write + 200.0 * write_12h) / 1e6;
        let cost = call.usd_on("gpt-6.1-sol", today).expect("priced");
        assert!((cost - expected).abs() < 1e-12, "{cost} vs {expected}");
    }

    /// A keep-alive ping reads the whole context at the cache read rate.
    #[test]
    fn a_call_prices_each_token_class_at_its_rate() {
        let today = crate::llm::model_prices::today();
        let table = crate::llm::model_prices::price_table();
        let rates = table
            .card_on("claude-opus-5-5", today)
            .expect("priced")
            .rates;
        let usd = |u: &ApiUsage| u.usd_on("claude-opus-5-5[1m]", today).expect("priced");
        let ping = usage(262_000, 5, 262_000, 0, 0);
        let expected = (262_000.0 * rates.cache_read + 5.0 * rates.out) / 1e6;
        assert!((usd(&ping) - expected).abs() < 1e-12);
        let fresh_and_writes = usage(1_300, 0, 0, 1_200, 200);
        let write_1h = rates.cache_write_1h.unwrap_or(rates.cache_write);
        let expected =
            (100.0 * rates.uncached_in + 1_000.0 * rates.cache_write + 200.0 * write_1h) / 1e6;
        assert!((usd(&fresh_and_writes) - expected).abs() < 1e-12);
        assert_eq!(ping.usd_on("my-local-model", today), None);
    }

    /// A prompt past the card's long threshold prices every token, output
    /// included, at the long rates.
    #[test]
    fn a_long_prompt_prices_at_the_long_tier() {
        let table = crate::llm::model_prices::price_table();
        let today = crate::llm::model_prices::today();
        let (model, card) = table
            .models
            .keys()
            .find_map(|model| {
                let card = table.card_on(model, today)?;
                card.long.map(|_| (model.clone(), card.clone()))
            })
            .expect("a shipped model with a long tier");
        let long = card.long.expect("long tier");
        let past = u32::try_from(long.threshold_tokens + 1).expect("fits");
        let call = usage(past, 10, 0, 0, 0);
        let expected = (f64::from(past) * long.rates.uncached_in + 10.0 * long.rates.out) / 1e6;
        let cost = call.usd_on(&model, today).expect("priced");
        assert!((cost - expected).abs() < 1e-12, "{cost} vs {expected}");
        let short = usage(1_000, 10, 0, 0, 0);
        let base = (1_000.0 * card.rates.uncached_in + 10.0 * card.rates.out) / 1e6;
        assert!((short.usd_on(&model, today).expect("priced") - base).abs() < 1e-12);
    }

    /// Every non-voice producer must keep writing the payload it always wrote.
    /// A `"modality": null` key would change every stored row for nothing.
    #[test]
    fn a_usage_block_with_no_split_grows_no_key() {
        let json = serde_json::to_value(ApiUsage {
            input_tokens: 12_345,
            output_tokens: 678,
            cache_read_tokens: 10_000,
            cache_creation_tokens: 200,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "input_tokens": 12_345,
                "output_tokens": 678,
                "cache_read_tokens": 10_000,
                "cache_creation_tokens": 200
            })
        );
    }

    /// Rows written before the field existed still read. They are the whole
    /// event store, so this is the one that must never break.
    #[test]
    fn a_stored_row_with_no_split_still_reads() {
        let stored = serde_json::json!({
            "input_tokens": 900,
            "output_tokens": 40,
            "cache_read_tokens": 800,
            "cache_creation_tokens": 0
        });
        let usage: ApiUsage = serde_json::from_value(stored).unwrap();
        assert_eq!(usage.input_tokens, 900);
        assert_eq!(usage.modality, None);
    }

    /// The split rides along under one key, so a reader that knows nothing
    /// about it reads the same four numbers it always did.
    #[test]
    fn a_split_serializes_under_one_key() {
        let json = serde_json::to_value(ApiUsage {
            input_tokens: 1200,
            output_tokens: 64,
            cache_read_tokens: 1024,
            cache_creation_tokens: 0,
            modality: Some(ModalityUsage {
                input_text_tokens: 176,
                input_audio_tokens: 1024,
                input_image_tokens: 0,
                cache_read_text_tokens: 100,
                cache_read_audio_tokens: 924,
                cache_read_image_tokens: 0,
                output_text_tokens: 20,
                output_audio_tokens: 44,
            }),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(json["input_tokens"], 1200);
        assert_eq!(json["modality"]["input_audio_tokens"], 1024);
        assert_eq!(json["modality"]["output_audio_tokens"], 44);
        let back: ApiUsage = serde_json::from_value(json).unwrap();
        assert_eq!(back.modality.unwrap().cache_read_audio_tokens, 924);
    }
}
