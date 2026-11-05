use super::*;

fn day(text: &str) -> NaiveDate {
    text.parse().expect("a YYYY-MM-DD day")
}

fn shipped_price(model: &str) -> ModelPrice {
    shipped_table()
        .price_on(model, today())
        .unwrap_or_else(|| panic!("{model} is not priced"))
}

/// A model priced twice is the failure the table exists to prevent.
#[test]
fn no_shipped_id_finds_two_rows() {
    let table = shipped_table();
    for id in table.models.keys() {
        let found = table
            .models
            .keys()
            .filter(|other| served_as_sent(other, id))
            .count();
        assert_eq!(found, 1, "{id} matches {found} rows");
    }
}

/// `[1m]`, `vendor/` and snapshot ids price as their base model, and a
/// later minor version does not price as its neighbour.
#[test]
fn recorded_model_ids_find_their_row() {
    let table = shipped_table();
    let input = |id: &str| table.price_on(id, today()).map(|p| p.input);
    for id in [
        "claude-opus-5-5",
        "claude-opus-5-5[1m]",
        "anthropic/claude-opus-5-5",
        "claude-opus-5-5@20260901",
        "claude-opus-5-5-20260901",
    ] {
        assert_eq!(input(id), input("claude-opus-5-5"), "{id}");
    }
    assert_ne!(input("claude-opus-5"), input("claude-opus-5-5"));
    assert_ne!(input("claude-fable-5-1"), None);
    assert_eq!(input("my-local-model"), None);
}

#[test]
fn an_unknown_model_pays_the_unpriced_shares() {
    let table = shipped_table();
    assert_eq!(
        cache_shares_of("my-local-model"),
        table.unpriced_cache_shares
    );
    assert_eq!(
        cache_shares_of("claude-opus-5-5[1m]"),
        shipped_price("claude-opus-5-5").cache
    );
}

/// The fallback is the shortest budget, so no Claude row may undercut its
/// write-to-read ratio at either TTL. Other providers' caches write at other
/// ratios, and the keep-alive never prices them.
#[test]
fn no_claude_row_undercuts_the_fallback() {
    let table = shipped_table();
    let ratios = |c: CacheShares| (c.write_5m / c.read, c.write_1h / c.read);
    let floor = ratios(table.unpriced_cache_shares);
    for id in table.models.keys().filter(|id| id.starts_with("claude-")) {
        let (five, hour) = ratios(shipped_price(id).cache);
        assert!(five >= floor.0 && hour >= floor.1, "{id}");
    }
}

/// The pricing page's ratios, which the keep-alive budget follows.
#[test]
fn claude_ratios_match_the_pricing_page() {
    for (id, (five, hour)) in [
        ("claude-fable-5-1", (50.0, 80.0)),
        ("claude-opus-5-5", (25.0, 40.0)),
        ("claude-sonnet-5-5", (25.0, 40.0)),
        ("claude-opus-5", (12.5, 20.0)),
        ("claude-haiku-4-5", (12.5, 20.0)),
    ] {
        let cache = shipped_price(id).cache;
        let (got_five, got_hour) = (cache.write_5m / cache.read, cache.write_1h / cache.read);
        assert!((got_five - five).abs() < 1e-9, "{id}: {got_five}");
        assert!((got_hour - hour).abs() < 1e-9, "{id}: {got_hour}");
    }
}

#[test]
fn every_shipped_card_names_its_source() {
    for (id, cards) in &shipped_table().models {
        for card in cards {
            assert!(!card.card.source.trim().is_empty(), "{id}");
        }
    }
}

/// A card applies from its `from` day, so a dated price change waits for its
/// day and history keeps the price it had.
#[test]
fn a_dated_card_applies_from_its_day() {
    let table = shipped_table();
    let flash = |text: &str| {
        table
            .price_on("gemini-3.8-flash", day(text))
            .map(|p| p.input)
    };
    let (intro, later) = (flash("2026-12-31"), flash("2027-01-01"));
    assert!(intro.is_some() && later.is_some());
    assert!(later > intro, "the 2027 card raises the price");
    assert_eq!(flash("2030-06-01"), later);
}

fn shipped_with(overlay: &str) -> PriceTable {
    merge(
        shipped_file(),
        Some(serde_json::from_str(overlay).map_err(|e| e.to_string())),
    )
}

fn input_on(table: &PriceTable, model: &str, text: &str) -> Option<f64> {
    table.price_on(model, day(text)).map(|p| p.input)
}

#[test]
fn with_no_override_the_shipped_cards_stand() {
    let table = merge(shipped_file(), None);
    assert_eq!(table, *shipped_table());
    assert!(!table.override_state.present);
    assert!(table
        .models
        .values()
        .flatten()
        .all(|c| c.origin == CardOrigin::Shipped));
}

/// A newer override card starts on its day. The days before it keep the
/// shipped price.
#[test]
fn a_newer_override_card_leaves_the_past_alone() {
    let before = input_on(&shipped_table(), "claude-opus-5-5", "2026-10-01");
    let table = shipped_with(
        r#"{"checked": "2026-11-02", "models": {"claude-opus-5-5": [
            {"from": "2026-11-01", "uncached_in": 3, "cache_write": 3.75,
             "cache_read": 0.15, "out": 15, "source": "a later price"}]}}"#,
    );
    assert_eq!(input_on(&table, "claude-opus-5-5", "2026-10-31"), before);
    assert_eq!(input_on(&table, "claude-opus-5-5", "2026-11-01"), Some(3.0));
    assert_eq!(table.override_state.checked, Some(day("2026-11-02")));
    let origins: Vec<_> = table.models["claude-opus-5-5"]
        .iter()
        .map(|c| c.origin)
        .collect();
    assert_eq!(origins, [CardOrigin::Shipped, CardOrigin::Override]);
}

/// The table reads checked on the newer of the shipped day and the
/// override's.
#[test]
fn the_table_reads_checked_on_the_newest_day() {
    let shipped = shipped_table().checked;
    assert_eq!(shipped_with(r#"{"models": {}}"#).checked, shipped);
    let older = shipped.pred_opt().expect("a day before");
    let newer = shipped.succ_opt().expect("a day after");
    let with_checked = |d: NaiveDate| shipped_with(&format!(r#"{{"checked": "{d}"}}"#)).checked;
    assert_eq!(with_checked(older), shipped);
    assert_eq!(with_checked(newer), newer);
}

/// An override card from the same day as a shipped one replaces it, so it
/// reprices that card's whole span.
#[test]
fn an_override_card_from_the_same_day_replaces_the_shipped_one() {
    let table = shipped_with(
        r#"{"models": {"claude-opus-5-5": [
            {"uncached_in": 4.5, "cache_write": 5.625, "cache_read": 0.225,
             "out": 22.5, "source": "a corrected price"}]}}"#,
    );
    let cards = &table.models["claude-opus-5-5"];
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].origin, CardOrigin::Override);
    assert_eq!(input_on(&table, "claude-opus-5-5", "2026-01-01"), Some(4.5));
}

/// An override card dated between two shipped cards joins between them.
#[test]
fn an_override_card_between_shipped_cards_joins_in_order() {
    let table = shipped_with(
        r#"{"models": {"gemini-3.8-flash": [
            {"from": "2026-12-01", "uncached_in": 1, "cache_write": 1,
             "cache_read": 0.1, "out": 5, "source": "an early rise"}]}}"#,
    );
    let flash = |text: &str| input_on(&table, "gemini-3.8-flash", text);
    assert_eq!(flash("2026-11-30"), Some(0.75));
    assert_eq!(flash("2026-12-15"), Some(1.0));
    assert_eq!(flash("2027-01-01"), Some(1.5));
}

#[test]
fn an_override_adds_a_model_the_shipped_table_lacks() {
    let table = shipped_with(
        r#"{"models": {"my-local-model": [
            {"uncached_in": 0, "cache_write": 0, "cache_read": 0, "out": 0,
             "source": "runs on my own machine"}]}}"#,
    );
    let price = table.price_on("my-local-model", today()).expect("priced");
    assert_eq!(price.input, 0.0);
    assert_eq!(price.cache, table.unpriced_cache_shares, "a free model");
}

/// A bad override is ignored whole, and says why, rather than half-applied.
#[test]
fn an_invalid_override_is_ignored_with_its_reason() {
    for (overlay, reason) in [
        (r#"{"currency": "NOK", "models": {}}"#, "currency"),
        (
            r#"{"models": {"claude-opus-5-5": [
                {"uncached_in": 1, "cache_write": 1, "cache_read": 1, "out": 1, "source": "a"},
                {"uncached_in": 2, "cache_write": 2, "cache_read": 2, "out": 2, "source": "b"}]}}"#,
            "same day",
        ),
        (
            r#"{"models": {"claude-opus-5-5": [
                {"uncached_in": -1, "cache_write": 1, "cache_read": 1, "out": 1, "source": "a"}]}}"#,
            "negative",
        ),
        (r#"{"models": "#, "EOF"),
    ] {
        let table = shipped_with(overlay);
        let error = table.override_state.error.clone().unwrap_or_default();
        assert!(error.contains(reason), "{overlay}: {error}");
        assert_eq!(table.models, shipped_table().models, "{overlay}");
    }
}

/// The override file is re-read when it changes, and its removal restores
/// the shipped cards.
#[test]
fn the_override_file_is_reread_on_change() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let path = workspace.path().join(OVERRIDE_PATH);
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
    let opus = |table: &PriceTable| table.price_on("claude-opus-5-5", today()).map(|p| p.input);
    let shipped = opus(&shipped_table());
    assert_eq!(opus(&price_table_for(workspace.path())), shipped);

    let card = |input: &str| {
        format!(
            r#"{{"models": {{"claude-opus-5-5": [{{"uncached_in": {input}, "cache_write": 1,
            "cache_read": 1, "out": 1, "source": "a test"}}]}}}}"#
        )
    };
    std::fs::write(&path, card("7")).expect("write");
    assert_eq!(opus(&price_table_for(workspace.path())), Some(7.0));
    std::fs::write(&path, card("8.5")).expect("rewrite");
    assert_eq!(opus(&price_table_for(workspace.path())), Some(8.5));
    std::fs::remove_file(&path).expect("remove");
    assert_eq!(opus(&price_table_for(workspace.path())), shipped);
}

/// The API serves the merged table with each card's origin and the
/// override's state, in Token Cost's card field names.
#[test]
fn the_table_serializes_with_provenance() {
    let table = shipped_with(
        r#"{"models": {"claude-opus-5-5": [
            {"from": "2026-11-01", "uncached_in": 3, "cache_write": 3.75,
             "cache_read": 0.15, "out": 15, "source": "a later price"}]}}"#,
    );
    let json = serde_json::to_value(&table).expect("serializes");
    let cards = &json["models"]["claude-opus-5-5"];
    assert_eq!(cards[0]["origin"], "shipped");
    assert_eq!(cards[1]["origin"], "override");
    assert_eq!(cards[1]["from"], "2026-11-01");
    assert_eq!(cards[1]["uncached_in"], 3.0);
    assert_eq!(json["override"]["path"], OVERRIDE_PATH);
    assert_eq!(json["override"]["present"], true);
    assert_eq!(
        json["models"]["claude-haiku-5-5"][0]["long"]["threshold_tokens"],
        100_000
    );
}

/// Every price is defined once, here. A second copy shows as a rate literal
/// divided by a million, a `ModelPrice` literal, or a JSON price key.
/// `lucidos-eval` pins its own run prices on purpose (its `prices.toml`).
#[test]
fn no_rate_literal_lives_outside_the_price_table() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let per_million =
        regex::Regex::new(r"\d+\.\d+\s*/\s*(1e6|1_000_000(\.0)?)\b").expect("a regex");
    let price_key = |key: &str| {
        key.to_ascii_lowercase().contains("price")
            || ["uncached_in", "cache_read", "cache_write", "cache_write_1h"].contains(&key)
    };
    let mut hits = Vec::new();
    let mut stack = vec![crates.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("a readable dir").flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                let skipped = ["target", "node_modules", "dist", "lucidos-eval"];
                if !skipped.contains(&name.as_str()) && !name.starts_with('.') {
                    stack.push(path);
                }
                continue;
            }
            let rel = path
                .strip_prefix(&crates)
                .unwrap_or(&path)
                .display()
                .to_string();
            if rel.ends_with("llm/model_prices.json") || rel.ends_with("llm/model_prices.rs") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if name.ends_with(".json") {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                    collect_price_keys(&value, &price_key, &rel, &mut hits);
                }
            } else if [".rs", ".ts", ".tsx"].iter().any(|ext| name.ends_with(ext)) {
                for (n, line) in text.lines().enumerate() {
                    let price_literal = line.contains("ModelPrice {") && !line.contains("->");
                    if per_million.is_match(line) || price_literal {
                        hits.push(format!("{rel}:{}: {}", n + 1, line.trim()));
                    }
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "rates outside the price table:\n{}",
        hits.join("\n")
    );
}

fn collect_price_keys(
    value: &serde_json::Value,
    price_key: &dyn Fn(&str) -> bool,
    rel: &str,
    hits: &mut Vec<String>,
) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, inner) in map {
                let numeric = inner.is_number()
                    || inner
                        .as_object()
                        .is_some_and(|m| m.values().any(serde_json::Value::is_number));
                if price_key(key) && numeric {
                    hits.push(format!("{rel}: \"{key}\""));
                }
                collect_price_keys(inner, price_key, rel, hits);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_price_keys(item, price_key, rel, hits);
            }
        }
        _ => {}
    }
}
