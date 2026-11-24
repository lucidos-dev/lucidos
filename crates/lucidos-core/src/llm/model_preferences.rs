//! Parsers for model-specific preferences.

use super::reasoning::EFFORT_LADDER;

/// Parse comma-separated `model=value` pairs, the shape of the per-model
/// preferences. `unit` names the value in errors, and `parse` reads one value.
pub(crate) fn parse_model_pairs<'a, T>(
    value: &'a str,
    unit: &str,
    parse: impl Fn(&str) -> Result<T, String>,
) -> Result<Vec<(&'a str, T)>, String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| {
            let (id, raw) = item
                .split_once('=')
                .ok_or_else(|| format!("'{item}' is not model={unit}"))?;
            let id = id.trim();
            if id.is_empty() {
                return Err(format!("'{item}' names no model"));
            }
            let parsed = parse(raw.trim()).map_err(|why| format!("'{item}' {why}"))?;
            Ok((id, parsed))
        })
        .collect()
}

/// Parse `model=tier` pairs, each tier on the ladder.
pub fn parse_model_efforts(value: &str) -> Result<Vec<(&str, &'static str)>, String> {
    parse_model_pairs(value, "tier", |raw| {
        crate::llm::reasoning::tier(raw)
            .ok_or_else(|| format!("has no tier; use one of {}", EFFORT_LADDER.join(", ")))
    })
}

/// Parse an *intelligence ladder*: `model=effort` rungs in order. Refuses an
/// empty list, a tier outside `efforts`, and a rung named twice. A `[1m]`
/// twin is the same rung as its base model.
pub fn parse_intelligence_ladder<'a>(
    value: &'a str,
    efforts: &'static [&'static str],
) -> Result<Vec<(&'a str, &'static str)>, String> {
    let pairs = parse_model_pairs(value, "effort", |raw| {
        efforts.iter().find(|e| **e == raw).copied().ok_or_else(|| {
            format!(
                "has no effort this ladder takes; use one of {}",
                efforts.join(", ")
            )
        })
    })?;
    if pairs.is_empty() {
        return Err("names no rung".to_string());
    }
    let mut seen = std::collections::HashSet::new();
    for (model, effort) in &pairs {
        let base = crate::llm::intelligence_rungs::intelligence_rung_model_base(model);
        if !seen.insert((base, *effort)) {
            return Err(format!("names {model}={effort} twice"));
        }
    }
    Ok(pairs)
}
