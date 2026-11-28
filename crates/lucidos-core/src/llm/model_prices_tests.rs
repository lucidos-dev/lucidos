use super::*;

fn day(text: &str) -> NaiveDate {
    text.parse().expect("a YYYY-MM-DD day")
}

fn shipped_price(model: &str) -> ModelPrice {
    shipped_table()
        .price_on(model, today())
        .unwrap_or_else(|| panic!("{model} is not priced"))
}

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

#[test]
fn november_models_use_standard_prices_and_december_boundary() {
    let table = shipped_table();
    for model in ["gemini-4-argon", "gemini-4-carbon"] {
        for (date, input, output, read) in [
            ("2026-11-30", 2.0, 10.0, 0.1),
            ("2026-12-01", 4.0, 20.0, 0.2),
        ] {
            let card = table.card_on(model, day(date)).unwrap();
            assert_eq!(card.rates.uncached_in, input);
            assert_eq!(card.rates.out, output);
            assert_eq!(card.rates.cache_read, read);
            assert_eq!(card.rates.cache_write, input);
        }
    }
    let card = table
        .card_on("claude-fable-5-2", day("2026-11-10"))
        .unwrap();
    assert_eq!(card.rates.uncached_in, 8.0);
    assert_eq!(card.rates.out, 40.0);
    assert_eq!(card.rates.cache_read, 0.2);
    assert_eq!(card.rates.cache_write, 10.0);
    assert_eq!(card.rates.cache_write_1h, Some(16.0));
}

#[test]
fn gemini_four_neon_uses_flat_standard_prices() {
    let table = shipped_table();
    for (date, write) in [
        ("2026-11-18", 3.0),
        ("2027-01-11", 3.0),
        ("2027-01-12", 3.75),
    ] {
        let card = table.card_on("gemini-4-neon", day(date)).unwrap();
        assert_eq!(card.rates.uncached_in, 3.0);
        assert_eq!(card.rates.out, 15.0);
        assert_eq!(card.rates.cache_read, 0.15);
        assert_eq!(card.rates.cache_write, write);
    }
}

#[test]
fn gpt_6_2_launch_cards_cover_cache_and_long_context() {
    let table = shipped_table();
    for (model, input) in [("gpt-6.2-astra", 8.0), ("gpt-6.2-sol", 2.0)] {
        let card = table.card_on(model, day("2026-11-17")).unwrap();
        assert_eq!(card.rates.uncached_in, input);
        assert_eq!(card.rates.out, input * 5.0);
        assert_eq!(card.rates.cache_read, input * 0.05);
        assert_eq!(card.rates.cache_write, input * 1.25);
        assert_eq!(card.rates.cache_write_12h, Some(input * 2.5));
        assert!(card.flex.is_none());
        let long = card.long.unwrap();
        assert_eq!(long.threshold_tokens, 272_000);
        assert!(!long.inclusive);
        assert_eq!(long.rates.uncached_in, input * 2.0);
        assert_eq!(long.rates.out, input * 7.5);
        assert_eq!(long.rates.cache_read, input * 0.1);
        assert_eq!(long.rates.cache_write, input * 2.5);
        assert_eq!(long.rates.cache_write_12h, Some(input * 5.0));
    }
}
