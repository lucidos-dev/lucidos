use super::*;

fn everything_offered(_: &str, _: &str) -> bool {
    true
}

fn pairs_of(ladder: &[IntelligenceRung]) -> Vec<String> {
    ladder.iter().map(IntelligenceRung::to_string).collect()
}

const CODING: [LadderSurface; 2] = [LadderSurface::ClaudeCode, LadderSurface::Codex];

fn opus_at(effort: &str) -> IntelligenceRung {
    IntelligenceRung::new("claude-opus-5-5", effort)
}

/// Every pair offered, labelled by id, on routes that bill per token.
struct OfferAll;

impl SurfaceOffer for OfferAll {
    fn offers(&self, _: &str, _: &str) -> bool {
        true
    }

    fn label(&self, model: &str) -> String {
        model.to_string()
    }

    fn is_free(&self, _: &str) -> bool {
        false
    }
}

fn share_of(share: f64) -> LongPromptShare {
    LongPromptShare {
        effective: share,
        set: None,
        measured: None,
        line_tokens: LONG_PROMPT_TOKENS,
    }
}

/// The served ladder with `reference` as the backend's default effort: Opus
/// on Claude Code, GPT-6.1 Sol on Codex.
pub(crate) fn served(
    surface: LadderSurface,
    override_value: Option<&str>,
    reference: &str,
    share: f64,
) -> IntelligenceLadder {
    let default = match surface {
        LadderSurface::Codex => IntelligenceRung::new("gpt-6.1-sol", reference),
        _ => opus_at(reference),
    };
    assemble(
        surface,
        &OfferAll,
        override_value,
        Some(default.clone()),
        Some(default),
        Some(share_of(share)),
    )
}

fn served_intelligence_rung<'a>(
    ladder: &'a IntelligenceLadder,
    model: &str,
    effort: &str,
) -> &'a ServedIntelligenceRung {
    ladder
        .intelligence_rungs
        .iter()
        .find(|r| r.model == model && r.effort == effort)
        .unwrap_or_else(|| panic!("{model}={effort} is not served"))
}

fn cost_label(ladder: &IntelligenceLadder, model: &str, effort: &str) -> String {
    served_intelligence_rung(ladder, model, effort)
        .cost
        .as_ref()
        .map(|b| b.label.clone())
        .unwrap_or_default()
}

fn intelligence_label(ladder: &IntelligenceLadder, model: &str, effort: &str) -> String {
    served_intelligence_rung(ladder, model, effort)
        .intelligence
        .as_ref()
        .map(|b| b.label.clone())
        .unwrap_or_default()
}

/// The refresh script writes bands on the grid the engine reads them on.
/// Python cannot import the constant, so this pins its copy.
#[test]
fn the_refresh_script_counts_bands_on_the_engine_grid() {
    let script = include_str!("../../../../.claude/skills/update-lucidos-models/estimate_rungs.py");
    for line in [
        format!("COST_STEPS_PER_DOUBLING = {COST_STEPS_PER_DOUBLING}\n"),
        format!("TIME_STEPS_PER_DOUBLING = {TIME_STEPS_PER_DOUBLING}\n"),
    ] {
        assert!(
            script.contains(&line),
            "estimate_rungs.py must say {line:?}"
        );
    }
}

#[test]
fn every_shipped_coding_pair_is_an_offered_menu_pair_under_a_dated_source() {
    let estimates = rung_estimates();
    assert!(!estimates.source.is_empty());
    assert!(
        chrono::NaiveDate::parse_from_str(&estimates.estimated, "%Y-%m-%d").is_ok(),
        "estimate date {}",
        estimates.estimated
    );
    for surface in CODING {
        let agent = surface.coding_agent().expect("a coding surface");
        assert!(!surface.shipped_pairs().is_empty());
        for p in surface.shipped_pairs() {
            assert!(
                prefs::CODING_AGENT_EFFORTS.contains(&p.effort.as_str()),
                "{}={} names no coding-agent effort",
                p.model,
                p.effort
            );
            assert!(
                is_offered(agent, &p.model, &p.effort),
                "{}={} is shipped but not offered by the {agent:?} menu",
                p.model,
                p.effort
            );
        }
    }
}

fn surfaces() -> [(&'static str, &'static [SurfacePair]); 3] {
    LadderSurface::ALL.map(|surface| (surface.as_wire(), surface.shipped_pairs()))
}

fn surface_named(name: &str) -> LadderSurface {
    LadderSurface::ALL
        .into_iter()
        .find(|s| s.as_wire() == name)
        .expect("a surface")
}

fn estimate_of(model: &str, effort: &str) -> Option<&'static EstimatedPair> {
    rung_estimates()
        .pairs
        .iter()
        .find(|e| e.model == model && e.effort == effort)
}

/// The day the estimates were made, which the frontier's prices are read on.
fn estimate_day() -> chrono::NaiveDate {
    chrono::NaiveDate::parse_from_str(&rung_estimates().estimated, "%Y-%m-%d")
        .expect("a dated estimate")
}

/// Cost per task at list price on the estimate day, in band steps.
fn list_cost_on_estimate_day(pair: &EstimatedPair) -> f64 {
    let price = model_prices::price_table()
        .price_on(&pair.model, estimate_day())
        .unwrap_or_else(|| panic!("{} has no price card", pair.model));
    f64::from(pair.token_band) + steps_of(price.output)
}

/// A pair the coding index lacks adds its model's mean measured gap to its
/// general band. It sits within a band of that model's measured gaps. A model
/// with no measured pair copies its general band, except that max reads as
/// xhigh, which may be a band lower.
fn estimated_gap_is_the_models_own(estimated: &EstimatedPair) -> bool {
    let gap = |e: &EstimatedPair| e.coding_band - e.general_band;
    let measured: Vec<i32> = rung_estimates()
        .pairs
        .iter()
        .filter(|e| e.model == estimated.model && !e.coding_estimated)
        .map(gap)
        .collect();
    match (measured.iter().min(), measured.iter().max()) {
        (Some(lowest), Some(highest)) => (lowest - 1..=highest + 1).contains(&gap(estimated)),
        _ => (-1..=0).contains(&gap(estimated)),
    }
}

#[test]
fn every_surface_pair_has_one_estimate_and_every_estimate_a_surface() {
    let estimates = &rung_estimates().pairs;
    for (i, e) in estimates.iter().enumerate() {
        assert!(
            !estimates[..i]
                .iter()
                .any(|earlier| earlier.model == e.model && earlier.effort == e.effort),
            "{}={} is estimated twice",
            e.model,
            e.effort
        );
        assert!(
            surfaces().iter().any(|(_, pairs)| pairs
                .iter()
                .any(|p| p.model == e.model && p.effort == e.effort)),
            "{}={} is estimated but on no surface",
            e.model,
            e.effort
        );
        assert!(
            !e.coding_estimated || estimated_gap_is_the_models_own(e),
            "{}={}: an estimated coding band keeps its model's measured gap",
            e.model,
            e.effort
        );
    }
    for (surface, pairs) in surfaces() {
        for (i, p) in pairs.iter().enumerate() {
            assert!(
                estimate_of(&p.model, &p.effort).is_some(),
                "{surface}: {}={} has no estimate",
                p.model,
                p.effort
            );
            assert!(
                !pairs[..i]
                    .iter()
                    .any(|earlier| earlier.model == p.model && earlier.effort == p.effort),
                "{surface}: {}={} is listed twice",
                p.model,
                p.effort
            );
        }
    }
}

/// No third-party figure ships. Every number in the file is a band, and every
/// field is one the engine reads. No Artificial Analysis field name appears
/// (ADR 0500, ADR 0527).
#[test]
fn the_rung_estimates_ship_integer_bands_only() {
    const FIELDS: &[&str] = &[
        "_note",
        "source",
        "estimated",
        "pairs",
        "surfaces",
        "chat",
        "claude_code",
        "codex",
        "model",
        "effort",
        "general_band",
        "coding_band",
        "coding_estimated",
        "token_band",
        "time_band",
        "on",
    ];
    fn bands_only(value: &serde_json::Value, path: &str) {
        match value {
            serde_json::Value::Number(n) => {
                assert!(n.is_i64(), "{path} is {n}, not an integer band");
            }
            serde_json::Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    bands_only(item, &format!("{path}[{i}]"));
                }
            }
            serde_json::Value::Object(fields) => {
                for (key, item) in fields {
                    assert!(FIELDS.contains(&key.as_str()), "{path} has a field {key}");
                    bands_only(item, &format!("{path}.{key}"));
                }
            }
            _ => {}
        }
    }
    let file: serde_json::Value = serde_json::from_str(RUNG_ESTIMATES_JSON).expect("valid JSON");
    bands_only(&file, "rung_estimates");
    for figure in [
        "intelligenceIndex",
        "indexScore",
        "costPerTask",
        "costUsd",
        "agentWallTimeSec",
        "outputTokens",
        "medianOutputSpeed",
    ] {
        assert!(
            !RUNG_ESTIMATES_JSON.contains(figure),
            "the rung estimates carry {figure}"
        );
    }
}

#[tokio::test]
async fn the_chat_surface_lists_current_builtin_models_at_their_tiers() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let models = crate::core::models::ModelStore::list(&pool)
        .await
        .expect("list models");
    for p in &rung_estimates().surfaces.chat {
        let model = models
            .iter()
            .find(|m| m.id == p.model)
            .unwrap_or_else(|| panic!("{} is on the chat surface but not a model", p.model));
        assert!(
            model.is_builtin() && model.successor.is_none(),
            "{} is on the chat surface but not a current builtin",
            p.model
        );
        assert!(
            model.routes.iter().any(|route| {
                let kind = crate::llm::model_registry::ProviderKind::parse(&route.provider);
                crate::llm::reasoning::supported_efforts(kind, route.wire_id(&model.id))
                    .contains(&p.effort.as_str())
            }),
            "{}={}: no route of the model takes that effort",
            p.model,
            p.effort
        );
    }
    crate::test_support::teardown_test_db(&db_name).await;
}

#[test]
fn each_surface_runs_least_to_most_intelligent_on_its_profile() {
    for (surface, pairs) in surfaces() {
        let bands: Vec<(&SurfacePair, i32)> = pairs
            .iter()
            .filter_map(|p| {
                estimate_of(&p.model, &p.effort)
                    .map(|e| (p, surface_named(surface).profile_band(e)))
            })
            .collect();
        assert!(
            bands.iter().all(|(_, band)| *band > 0),
            "{surface}: a pair has no intelligence band"
        );
        for pair in bands.windows(2) {
            assert!(
                pair[0].1 <= pair[1].1,
                "{surface}: {}={} is less intelligent than the pair before it",
                pair[1].0.model,
                pair[1].0.effort
            );
        }
    }
}

#[test]
fn the_effort_vocabulary_is_both_menus_effort_rows() {
    for agent in [CodingAgent::ClaudeCode, CodingAgent::Codex] {
        let menu: Vec<&str> = super::super::coding_agent_reasoning_effort_options(agent)
            .iter()
            .map(|o| o.value.as_str())
            .collect();
        assert_eq!(menu, prefs::CODING_AGENT_EFFORTS, "{agent:?}");
    }
}

#[test]
fn each_surface_s_pairs_that_are_on_form_a_frontier() {
    for (surface, pairs) in surfaces() {
        let cost =
            |p: &SurfacePair| estimate_of(&p.model, &p.effort).map(list_cost_on_estimate_day);
        let on: Vec<&SurfacePair> = pairs.iter().filter(|p| p.on).collect();
        // A pair the coding index never measured is kept on its general
        // score, so only measured pairs share the coding order.
        let measured_on_profile = |p: &&SurfacePair| {
            surface == "chat"
                || estimate_of(&p.model, &p.effort).is_some_and(|e| !e.coding_estimated)
        };
        let ranked_on: Vec<&SurfacePair> = on.iter().copied().filter(measured_on_profile).collect();
        for pair in ranked_on.windows(2) {
            assert!(
                cost(pair[0]) <= cost(pair[1]),
                "{surface}: {}={} costs more than the smarter {}={}",
                pair[0].model,
                pair[0].effort,
                pair[1].model,
                pair[1].effort
            );
        }
        // An equally smart pair may sort before the pair it beats.
        for off in pairs.iter().filter(|p| !p.on) {
            assert!(
                on.iter().any(|p| cost(p) <= cost(off)),
                "{surface}: no pair that is on costs at most {}={}",
                off.model,
                off.effort
            );
        }
    }
}

#[test]
fn the_claude_code_ladder_climbs_haiku_then_sonnet_and_opus_to_opus_max() {
    let ladder = effective_ladder(LadderSurface::ClaudeCode, None, everything_offered);
    assert_eq!(
        pairs_of(&ladder),
        [
            "claude-haiku-5-5=low",
            "claude-haiku-5-5=medium",
            "claude-haiku-5-5=high",
            "claude-haiku-5-5=xhigh",
            "claude-sonnet-5-5=medium",
            "claude-opus-5-5=low",
            "claude-sonnet-5-5=high",
            "claude-opus-5-5=medium",
            "claude-opus-5-5=high",
            "claude-opus-5-5=xhigh",
            "claude-opus-5-5=max",
        ]
    );
}

#[test]
fn the_codex_ladder_climbs_luna_then_sol_to_sol_xhigh() {
    let ladder = effective_ladder(LadderSurface::Codex, None, everything_offered);
    assert_eq!(
        pairs_of(&ladder),
        [
            "gpt-6-luna=low",
            "gpt-5.6-luna=medium",
            "gpt-6-luna=medium",
            "gpt-6-luna=high",
            "gpt-6-luna=xhigh",
            "gpt-6-luna=max",
            "gpt-6.1-sol=low",
            "gpt-6.1-sol=medium",
            "gpt-6.1-sol=xhigh",
        ]
    );
}

#[test]
fn an_unoffered_intelligence_rung_drops_and_the_rest_keep_their_order() {
    let no_haiku = |model: &str, _: &str| model != "claude-haiku-5-5";
    let ladder = effective_ladder(LadderSurface::ClaudeCode, None, no_haiku);
    assert_eq!(
        pairs_of(&ladder)[..2],
        ["claude-sonnet-5-5=medium", "claude-opus-5-5=low"],
        "the shipped ladder is stored, so no rung Haiku beat comes back"
    );
}

#[test]
fn an_override_keeps_its_order_and_drops_what_is_not_offered() {
    let stored = "gpt-5.5=max, gpt-6.1-sol=high, gpt-9=low, gpt-6-luna=high, gpt-5.4=low";
    let ladder = effective_ladder(LadderSurface::Codex, Some(stored), |model, effort| {
        is_offered(CodingAgent::Codex, model, effort)
    });
    assert_eq!(
        pairs_of(&ladder),
        ["gpt-6.1-sol=high", "gpt-6-luna=high", "gpt-5.4=low"],
        "gpt-5.5 takes no max, gpt-9 is not offered, and the order is the owner's"
    );
}

/// Copy from Lucidos Agent keeps only what the backend offers, ranks it on the
/// coding profile, and puts an unbanded pair last.
#[test]
fn a_copy_from_chat_keeps_offered_pairs_in_coding_order() {
    let chat_on = vec![
        opus_at("max"),
        IntelligenceRung::new("gemini-3.8-flash", "high"),
        IntelligenceRung::new("my-model", "high"),
        IntelligenceRung::new("claude-haiku-5-5", "high"),
        opus_at("max"),
    ];
    let no_gemini = |model: &str, _: &str| !model.starts_with("gemini");
    let copied = copied_from_chat(LadderSurface::ClaudeCode, &chat_on, no_gemini);
    assert_eq!(
        pairs_of(&copied),
        [
            "claude-haiku-5-5=high",
            "claude-opus-5-5=max",
            "my-model=high"
        ]
    );
}

/// The shipped chat ladder copied onto each coding agent keeps only that
/// backend's pairs, least to most intelligent on its profile.
#[test]
fn the_shipped_chat_ladder_copies_onto_each_coding_agent() {
    let chat_on = effective_ladder(LadderSurface::Chat, None, everything_offered);
    for surface in CODING {
        let agent = surface.coding_agent().expect("a coding surface");
        let copied = copied_from_chat(surface, &chat_on, |m, e| is_offered(agent, m, e));
        assert!(!copied.is_empty(), "{surface:?}");
        assert!(copied
            .iter()
            .all(|p| is_offered(agent, &p.model, &p.effort)));
        let bands: Vec<i32> = copied
            .iter()
            .map(|p| surface.profile_band(estimate_for(surface, p).expect("an estimate")))
            .collect();
        assert!(bands.is_sorted(), "{surface:?}: {bands:?}");
    }
}

fn haiku_at(effort: &str) -> IntelligenceRung {
    IntelligenceRung::new("claude-haiku-5-5", effort)
}

/// Coding bands: haiku low 16, medium 17, high 18, xhigh 20 and sonnet low 20.
#[test]
fn a_ladder_climbs_when_no_estimated_intelligence_rung_sits_below_the_one_before() {
    let climbs_on = |on: &[IntelligenceRung]| climbs(LadderSurface::ClaudeCode, on);
    assert!(climbs_on(&[]));
    assert!(climbs_on(&[
        haiku_at("low"),
        haiku_at("medium"),
        haiku_at("high")
    ]));
    assert!(!climbs_on(&[haiku_at("high"), haiku_at("medium")]));
    assert!(
        !climbs_on(&[
            IntelligenceRung::new("claude-sonnet-5-5", "low"),
            haiku_at("high")
        ]),
        "the coding profile ranks sonnet low above haiku high"
    );
}

#[test]
fn equal_bands_and_unestimated_intelligence_rungs_never_break_the_climb() {
    let sonnet_low = IntelligenceRung::new("claude-sonnet-5-5", "low");
    let unestimated = IntelligenceRung::new("my-model", "high");
    for tied in [
        [haiku_at("xhigh"), sonnet_low.clone()],
        [sonnet_low.clone(), haiku_at("xhigh")],
    ] {
        assert!(climbs(LadderSurface::ClaudeCode, &tied), "{tied:?}");
    }
    for on in [
        [haiku_at("medium"), unestimated.clone(), haiku_at("high")],
        [unestimated.clone(), haiku_at("medium"), haiku_at("high")],
    ] {
        assert!(climbs(LadderSurface::ClaudeCode, &on), "{on:?}");
    }
}

#[test]
fn the_shipped_ladders_climb_on_their_own_profile() {
    for surface in LadderSurface::ALL {
        let on = effective_ladder(surface, None, everything_offered);
        assert!(climbs(surface, &on), "{surface:?}");
    }
}

#[test]
fn sorting_by_intelligence_is_stable_and_keeps_the_same_intelligence_rungs() {
    let sonnet_low = IntelligenceRung::new("claude-sonnet-5-5", "low");
    let unestimated = IntelligenceRung::new("my-model", "high");
    let on = vec![
        unestimated.clone(),
        sonnet_low.clone(),
        haiku_at("high"),
        haiku_at("xhigh"),
        haiku_at("low"),
    ];
    let sorted = sorted_by_intelligence(LadderSurface::ClaudeCode, &on);
    assert_eq!(
        pairs_of(&sorted),
        [
            "claude-haiku-5-5=low",
            "claude-haiku-5-5=high",
            "claude-sonnet-5-5=low",
            "claude-haiku-5-5=xhigh",
            "my-model=high"
        ],
        "tied rungs keep the owner's order and an unestimated rung comes last"
    );
    assert!(climbs(LadderSurface::ClaudeCode, &sorted));
    let mut before = pairs_of(&on);
    let mut after = pairs_of(&sorted);
    before.sort();
    after.sort();
    assert_eq!(before, after);
}

#[test]
fn the_served_ladder_says_whether_its_intelligence_rungs_climb() {
    assert!(served(LadderSurface::ClaudeCode, None, "medium", 0.0).climbs);
    let general_order = "claude-sonnet-5-5=low, claude-haiku-5-5=high";
    let ladder = served(
        LadderSurface::ClaudeCode,
        Some(general_order),
        "medium",
        0.0,
    );
    assert!(ladder.customised);
    assert!(
        !ladder.climbs,
        "the coding profile ranks sonnet low above haiku high"
    );
}

#[test]
fn the_served_intelligence_rungs_show_every_shipped_pair_with_the_off_ones_marked() {
    let ladder = served(LadderSurface::ClaudeCode, None, "medium", 0.0);
    assert_eq!(
        ladder.intelligence_rungs.len(),
        LadderSurface::ClaudeCode.shipped_pairs().len()
    );
    assert_eq!(ladder.pairs(), ladder.shipped_intelligence_rungs);
    assert!(!served_intelligence_rung(&ladder, "claude-sonnet-5-5", "low").on);
    assert!(served_intelligence_rung(&ladder, "claude-sonnet-5-5", "high").on);
    assert!(!ladder.customised);
}

#[test]
fn the_prompt_bar_lists_only_the_on_intelligence_rungs_without_a_context_twin() {
    for surface in CODING {
        let ladder = served(surface, None, "medium", 0.0);
        let listed = ladder.on_intelligence_rungs();
        assert_eq!(
            listed
                .iter()
                .map(|r| IntelligenceRung::new(&r.model, &r.effort))
                .collect::<Vec<_>>(),
            ladder.pairs(),
            "{surface:?}: the list is the effective ladder's on rungs, in order"
        );
        assert!(
            listed.iter().all(|r| !r.model.ends_with(CONTEXT_VARIANT)),
            "{surface:?}: no 1M twin is listed"
        );
        assert!(listed.iter().all(|r| !r.label.is_empty()));
    }
}

#[test]
fn the_prompt_bar_intelligence_rungs_carry_the_bands_the_ladder_route_serves() {
    for surface in CODING {
        let ladder = served(surface, None, "medium", 0.0);
        let on: Vec<_> = ladder.intelligence_rungs.iter().filter(|r| r.on).collect();
        for (listed, served) in ladder.on_intelligence_rungs().iter().zip(on) {
            assert_eq!(listed.intelligence, served.intelligence, "{surface:?}");
            assert_eq!(listed.cost, served.cost, "{surface:?}");
            assert_eq!(listed.time, served.time, "{surface:?}");
        }
    }
}

#[test]
fn an_override_decides_which_intelligence_rungs_the_prompt_bar_lists() {
    let ladder = served(
        LadderSurface::ClaudeCode,
        Some("claude-sonnet-5-5=low,claude-opus-5-5=high"),
        "medium",
        0.0,
    );
    assert_eq!(
        ladder
            .on_intelligence_rungs()
            .iter()
            .map(|r| format!("{}={}", r.model, r.effort))
            .collect::<Vec<_>>(),
        ["claude-sonnet-5-5=low", "claude-opus-5-5=high"]
    );
}

#[test]
fn under_an_override_an_off_pair_follows_the_shipped_pair_before_it() {
    let ladder = served(
        LadderSurface::ClaudeCode,
        Some("claude-opus-5-5=max, claude-haiku-5-5=low"),
        "medium",
        0.0,
    );
    let order: Vec<String> = ladder
        .intelligence_rungs
        .iter()
        .map(|r| format!("{}={}{}", r.model, r.effort, if r.on { "" } else { " off" }))
        .collect();
    assert_eq!(
        order[..4],
        [
            "claude-opus-5-5=max",
            "claude-haiku-5-5=low",
            "claude-haiku-5-5=medium off",
            "claude-haiku-5-5=high off",
        ]
    );
    assert_eq!(order.len(), 15, "{order:?}");
    assert!(ladder.customised);
}

#[test]
fn a_one_million_twin_is_the_same_intelligence_rung_as_its_base_model() {
    let ladder = served(LadderSurface::ClaudeCode, None, "medium", 0.0);
    let base = ladder.position_of(&opus_at("medium"));
    assert!(base.is_some());
    assert_eq!(
        ladder.position_of(&IntelligenceRung::new("claude-opus-5-5[1m]", "medium")),
        base
    );
    assert!(ladder
        .check_pick(&IntelligenceRung::new("claude-sonnet-5-5[1m]", "medium"))
        .is_ok());
}

#[test]
fn an_off_intelligence_rung_is_served_but_refused_naming_every_one_that_is_on() {
    let ladder = served(LadderSurface::ClaudeCode, None, "medium", 0.0);
    let off = IntelligenceRung::new("claude-sonnet-5-5", "max");
    assert!(ladder.position_of(&off).is_some());
    let err = ladder.check_pick(&off).unwrap_err();
    assert!(err.contains("claude-sonnet-5-5=max is not a rung"), "{err}");
    assert!(err.contains("claude-opus-5-5=max"), "{err}");
    assert!(!err.contains("claude-sonnet-5-5=low"), "{err}");
    assert!(err.contains("lucidos intelligence-ladder"), "{err}");
}

#[test]
fn an_on_position_counts_only_the_intelligence_rungs_that_are_on() {
    let ladder = served(LadderSurface::ClaudeCode, None, "medium", 0.0);
    let pairs = ladder.pairs();
    for (at, pair) in pairs.iter().enumerate() {
        assert_eq!(ladder.on_position_of(pair), Some(at), "{pair}");
    }
    assert_eq!(
        ladder.on_position_of(&IntelligenceRung::new("claude-sonnet-5-5", "max")),
        None
    );
}

#[test]
fn a_time_band_reads_as_a_rounded_multiple() {
    let labels: Vec<String> = [-17, -4, -3, -2, -1, 0, 1, 4, 6, 7]
        .into_iter()
        .map(|steps| Band::from_time_steps(steps).label)
        .collect();
    assert_eq!(
        labels,
        ["0.02×", "0.4×", "0.5×", "0.6×", "0.8×", "1.0×", "1.3×", "2.5×", "4.0×", "5.0×"]
    );
    assert_eq!(Band::from_time_steps(3).multiple, 2.0);
}

#[test]
fn a_cost_band_reads_on_the_finer_grid() {
    let labels: Vec<String> = [-6, -2, -1, 0, 1, 2, 3, 6]
        .into_iter()
        .map(|steps| Band::from_cost_steps(steps).label)
        .collect();
    assert_eq!(
        labels,
        ["0.5×", "0.8×", "0.9×", "1.0×", "1.1×", "1.3×", "1.4×", "2.0×"]
    );
    assert_eq!(Band::from_cost_steps(6).multiple, 2.0);
}

/// Astra costs about 15% more per task than Opus at `medium`, and the cost
/// band shows it.
#[test]
fn a_cost_within_a_quarter_of_the_reference_still_shows() {
    let chat = served(LadderSurface::Chat, None, "medium", 0.0);
    assert_eq!(cost_label(&chat, "gpt-6-astra", "medium"), "1.1×");
}

#[test]
fn an_intelligence_band_reads_in_hundredths() {
    let labels: Vec<String> = [(15, 25), (24, 25), (25, 25), (26, 25), (29, 25), (3, 100)]
        .into_iter()
        .map(|(band, reference)| Band::from_intelligence(band, reference).unwrap().label)
        .collect();
    assert_eq!(labels, ["0.6×", "0.96×", "1.0×", "1.04×", "1.16×", "0.03×"]);
    assert_eq!(Band::from_intelligence(5, 0), None);
    assert_eq!(Band::from_intelligence(0, 10), None);
}

#[test]
fn the_bands_follow_the_reference_intelligence_rung() {
    let at_medium = served(LadderSurface::ClaudeCode, None, "medium", 0.0);
    assert_eq!(cost_label(&at_medium, "claude-opus-5-5", "medium"), "1.0×");
    assert_eq!(cost_label(&at_medium, "claude-opus-5-5", "max"), "4.5×");
    assert_eq!(cost_label(&at_medium, "claude-haiku-5-5", "low"), "0.02×");
    assert_eq!(
        intelligence_label(&at_medium, "claude-opus-5-5", "medium"),
        "1.0×"
    );
    assert_eq!(
        intelligence_label(&at_medium, "claude-opus-5-5", "max"),
        "1.12×"
    );
    assert_eq!(
        intelligence_label(&at_medium, "claude-haiku-5-5", "low"),
        "0.64×"
    );
    let at_high = served(LadderSurface::ClaudeCode, None, "high", 0.0);
    assert_eq!(cost_label(&at_high, "claude-opus-5-5", "high"), "1.0×");
    assert_eq!(cost_label(&at_high, "claude-opus-5-5", "medium"), "0.8×");
    assert_eq!(
        intelligence_label(&at_high, "claude-opus-5-5", "high"),
        "1.0×"
    );
    assert_eq!(
        intelligence_label(&at_high, "claude-opus-5-5", "medium"),
        "0.93×"
    );
    assert!(served_intelligence_rung(&at_high, "claude-opus-5-5", "high").is_default);
    assert_eq!(at_high.reference, Some(opus_at("high")));
}

/// On the general profile only: the Coding Agent Index scores some higher
/// efforts lower, and the coding ladders show that (ADR 0527).
#[test]
fn a_models_higher_efforts_read_as_more_intelligent() {
    let ladder = served(LadderSurface::Chat, None, "medium", 0.0);
    for (model, efforts) in [
        ("gpt-6.1-sol", ["medium", "high", "xhigh"]),
        ("claude-opus-5-5", ["high", "xhigh", "max"]),
    ] {
        assert_efforts_rise(&ladder, model, &efforts);
    }
}

/// The fill never rewrites a pair the coding index measured.
#[test]
fn a_measured_coding_score_is_never_filled_over() {
    let max = estimate_of("claude-haiku-5-5", "max").expect("Haiku max is estimated");
    assert!(!max.coding_estimated, "the index measured Haiku at max");
    assert!(
        max.coding_band < max.general_band,
        "the measured coding dip stays: {max:?}"
    );
}

/// An unmeasured pair keeps its model's measured gap, so Opus xhigh never
/// reads level with high or past max on the coding profile.
#[test]
fn the_coding_ladder_keeps_a_models_unmeasured_efforts_rising() {
    let ladder = served(LadderSurface::ClaudeCode, None, "medium", 0.0);
    assert_efforts_rise(
        &ladder,
        "claude-opus-5-5",
        &["low", "medium", "high", "xhigh"],
    );
    let opus_band = |effort: &str| {
        served_intelligence_rung(&ladder, "claude-opus-5-5", effort)
            .intelligence
            .clone()
    };
    assert_eq!(
        opus_band("max"),
        opus_band("xhigh"),
        "Opus max reads as xhigh"
    );
    let codex = served(LadderSurface::Codex, None, "medium", 0.0);
    for model in ["gpt-6-astra", "gpt-5.6-luna"] {
        assert_efforts_rise(&codex, model, &["low", "medium", "high", "xhigh", "max"]);
    }
}

fn assert_efforts_rise(ladder: &IntelligenceLadder, model: &str, efforts: &[&str]) {
    let bands: Vec<&Band> = efforts
        .iter()
        .map(|effort| {
            served_intelligence_rung(ladder, model, effort)
                .intelligence
                .as_ref()
                .unwrap()
        })
        .collect();
    assert!(
        bands
            .windows(2)
            .all(|w| w[0].multiple < w[1].multiple && w[0].label != w[1].label),
        "{model}: {bands:?}"
    );
}

#[test]
fn codex_compares_against_its_own_default() {
    let ladder = served(LadderSurface::Codex, None, "medium", 0.0);
    assert_eq!(
        ladder.reference,
        Some(IntelligenceRung::new("gpt-6.1-sol", "medium"))
    );
    assert_eq!(cost_label(&ladder, "gpt-6.1-sol", "medium"), "1.0×");
    assert!(served_intelligence_rung(&ladder, "gpt-6.1-sol", "medium").is_default);
}

#[test]
fn a_pair_with_no_estimate_has_no_band() {
    let ladder = served(LadderSurface::Codex, Some("gpt-5.4=low"), "medium", 0.0);
    let only = served_intelligence_rung(&ladder, "gpt-5.4", "low");
    assert!(only.on);
    assert_eq!(only.intelligence, None);
    assert_eq!(only.cost, None);
    assert_eq!(only.time, None);
}

#[test]
fn the_long_prompt_share_prices_in_only_a_model_with_a_long_tier() {
    let base = served(LadderSurface::ClaudeCode, None, "medium", 0.0);
    let long = served(LadderSurface::ClaudeCode, None, "medium", 0.8);
    assert_eq!(cost_label(&long, "claude-haiku-5-5", "low"), "0.08×");
    assert_eq!(cost_label(&long, "claude-haiku-5-5", "max"), "0.6×");
    assert_eq!(cost_label(&long, "claude-sonnet-5-5", "high"), "0.6×");
    assert_eq!(
        served_intelligence_rung(&long, "claude-haiku-5-5", "low").time,
        served_intelligence_rung(&base, "claude-haiku-5-5", "low").time,
        "price moves cost, never time"
    );
}

#[tokio::test]
async fn the_measured_share_counts_claude_code_calls_past_the_line() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    assert_eq!(measure_long_prompt_share(&pool).await, None);
    for (producer, tokens) in [
        ("claude_code", 150_000),
        ("claude_code", 40_000),
        ("claude_code", 120_000),
        ("claude_code", 90_000),
        ("main_llm", 300_000),
    ] {
        sqlx::query(
            "INSERT INTO events (id, aggregate, event_type, payload) VALUES \
             (gen_random_uuid(), 'thread', 'ContextCaptured', \
              jsonb_build_object('producer', $1::text, 'usage', jsonb_build_object('input_tokens', $2::bigint)))",
        )
        .bind(producer)
        .bind(tokens)
        .execute(&pool)
        .await
        .expect("seed a capture");
    }
    assert_eq!(measure_long_prompt_share(&pool).await, Some(0.5));
    crate::test_support::teardown_test_db(&db_name).await;
}

fn opus_medium() -> DefaultImplementer {
    DefaultImplementer {
        model: "claude-opus-5-5".into(),
        effort: Some("medium".into()),
    }
}

#[test]
fn no_pick_takes_the_default_implementer() {
    assert_eq!(
        fill_pick(
            CodingAgent::ClaudeCode,
            None,
            None,
            Some(&opus_medium()),
            |_| None
        ),
        (Some("claude-opus-5-5".into()), Some("medium".into()))
    );
}

#[test]
fn an_effort_only_pick_keeps_the_default_model() {
    assert_eq!(
        fill_pick(
            CodingAgent::ClaudeCode,
            None,
            Some("high".into()),
            Some(&opus_medium()),
            |_| None
        ),
        (Some("claude-opus-5-5".into()), Some("high".into()))
    );
}

#[test]
fn a_model_only_pick_runs_at_the_models_default_effort() {
    let registry = |model: &str| (model == "claude-sonnet-5-5").then_some("high");
    assert_eq!(
        fill_pick(
            CodingAgent::ClaudeCode,
            Some("claude-sonnet-5-5".into()),
            None,
            Some(&opus_medium()),
            registry
        ),
        (Some("claude-sonnet-5-5".into()), Some("high".into()))
    );
}

#[test]
fn a_registry_tier_claude_code_does_not_take_is_not_pinned() {
    assert_eq!(
        fill_pick(
            CodingAgent::ClaudeCode,
            Some("claude-opus-4-8@default".into()),
            None,
            None,
            |_| Some("none")
        ),
        (Some("claude-opus-4-8@default".into()), None)
    );
}

fn sol_medium() -> DefaultImplementer {
    DefaultImplementer {
        model: "gpt-6.1-sol".into(),
        effort: Some("medium".into()),
    }
}

#[test]
fn a_codex_spawn_with_no_pick_takes_the_codex_default_implementer() {
    assert_eq!(
        fill_pick(CodingAgent::Codex, None, None, Some(&sol_medium()), |_| {
            None
        }),
        (Some("gpt-6.1-sol".into()), Some("medium".into()))
    );
}

#[test]
fn a_codex_model_only_pick_runs_at_the_models_codex_default_effort() {
    let registry = model_registry::empty();
    let filled = |model: &str| {
        fill_pick(
            CodingAgent::Codex,
            Some(model.into()),
            None,
            Some(&sol_medium()),
            |m| model_default_effort(CodingAgent::Codex, &registry, m),
        )
    };
    assert_eq!(
        filled("gpt-6.1-sol"),
        (Some("gpt-6.1-sol".into()), Some("low".into()))
    );
    assert_eq!(
        filled("gpt-6-luna"),
        (Some("gpt-6-luna".into()), Some("medium".into()))
    );
    assert_eq!(
        filled("default"),
        (Some("default".into()), None),
        "the sentinel leaves the effort to Codex's config"
    );
}

#[test]
fn every_codex_model_names_an_effort_it_accepts() {
    for row in super::super::codex::codex_model_options()
        .iter()
        .filter(|row| row.value != "default")
    {
        let effort = row
            .default_effort
            .as_deref()
            .unwrap_or_else(|| panic!("{} has no default_effort", row.value));
        assert!(
            super::super::validate_coding_agent_effort(
                CodingAgent::Codex,
                Some(&row.value),
                Some(effort)
            )
            .is_ok(),
            "{}: {effort} is not an effort it accepts",
            row.value
        );
    }
}

#[test]
fn codex_with_no_offered_default_picks_nothing() {
    assert_eq!(
        fill_pick(CodingAgent::Codex, None, None, None, |_| None),
        (None, None)
    );
}

#[tokio::test]
async fn the_shipped_codex_default_is_an_intelligence_rung_on_the_shipped_codex_ladder() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let routes = seeded_routes(&pool).await;
    assert_eq!(
        default_implementer(&pool, CodingAgent::Codex).await,
        Some(sol_medium())
    );
    let ladder = load(&pool, &routes, LadderSurface::Codex).await;
    let default = ladder
        .default_intelligence_rung
        .clone()
        .expect("the shipped Codex default is a rung");
    assert!(
        ladder.intelligence_rungs.iter().any(|r| r.on
            && r.is_default
            && r.model == default.model
            && r.effort == default.effort),
        "the default reads as an on rung of the ladder"
    );
    assert_eq!(
        ladder.reference.as_ref(),
        Some(&default),
        "Codex bands compare against Codex's own default"
    );
    crate::test_support::teardown_test_db(&db_name).await;
}

#[test]
fn a_default_the_backend_no_longer_offers_reads_as_none() {
    let stored = |model: &str, effort: &str| DefaultImplementer {
        model: model.into(),
        effort: Some(effort.into()),
    };
    assert_eq!(
        offered_default(CodingAgent::ClaudeCode, stored("claude-opus-55", "medium")),
        None,
        "a typo must not pin every spawn to a model Claude Code refuses"
    );
    assert_eq!(
        offered_default(CodingAgent::Codex, stored("gpt-5.5", "max")),
        Some(DefaultImplementer {
            model: "gpt-5.5".into(),
            effort: None
        }),
        "an effort the model does not take is dropped, the model kept"
    );
    assert_eq!(
        offered_default(CodingAgent::ClaudeCode, stored("claude-opus-5-5", "medium")),
        Some(stored("claude-opus-5-5", "medium"))
    );
}

#[test]
fn every_estimated_model_has_a_price_card() {
    for pair in &rung_estimates().pairs {
        assert!(
            model_prices::price_table()
                .price_on(&pair.model, estimate_day())
                .is_some(),
            "{} is estimated but has no price card, so its cost cannot be computed",
            pair.model
        );
    }
}

const HAIKU: &str = "claude-haiku-5-5";

/// A registry holding only Haiku 5.5, on four routes.
fn haiku_registry() -> ModelRegistry {
    let registry = model_registry::empty();
    let routing = model_registry::ModelRouting {
        routes: vec![
            model_registry::RouteEntry::new(ProviderKind::Vertex, HAIKU),
            model_registry::RouteEntry::new(ProviderKind::Anthropic, HAIKU),
            model_registry::RouteEntry::new(ProviderKind::ClaudeGateway, HAIKU),
            model_registry::RouteEntry::new(ProviderKind::OpenRouter, "anthropic/claude-haiku-5-5"),
            model_registry::RouteEntry::new(ProviderKind::OpenCodeFree, HAIKU),
        ],
        default_effort: Some("medium"),
        ..model_registry::ModelRouting::single(ProviderKind::Vertex, HAIKU)
    };
    registry
        .write()
        .expect("registry lock")
        .insert(HAIKU.to_string(), routing);
    registry
}

/// The chat ladder with Haiku 5.5 reached through `route` alone.
fn haiku_chat_ladder(route: ProviderKind) -> IntelligenceLadder {
    let offer = ChatOffer {
        routes: ModelRoutes {
            registry: haiku_registry(),
            configured: vec![route],
        },
        labels: HashMap::from([(HAIKU.to_string(), "Haiku 5.5".to_string())]),
    };
    assemble(
        LadderSurface::Chat,
        &offer,
        None,
        None,
        Some(opus_at("medium")),
        None,
    )
}

#[test]
fn a_route_never_splits_an_intelligence_rung_and_only_a_free_one_reads_included() {
    let routes = [
        ProviderKind::Vertex,
        ProviderKind::Anthropic,
        ProviderKind::OpenRouter,
        ProviderKind::ClaudeGateway,
        ProviderKind::OpenCodeFree,
    ];
    let ladders = routes.map(haiku_chat_ladder);
    let shipped_haiku: Vec<&str> = LadderSurface::Chat
        .shipped_pairs()
        .iter()
        .filter(|p| p.model == HAIKU)
        .map(|p| p.effort.as_str())
        .collect();
    for (route, ladder) in routes.iter().zip(&ladders) {
        let efforts: Vec<&str> = ladder
            .intelligence_rungs
            .iter()
            .map(|r| {
                assert_eq!(r.model, HAIKU, "{route:?}: only Haiku is offered");
                r.effort.as_str()
            })
            .collect();
        assert_eq!(efforts, shipped_haiku, "{route:?}: one row per effort");
        assert!(ladder
            .intelligence_rungs
            .iter()
            .all(|r| r.label == "Haiku 5.5"));
    }
    let [vertex, anthropic, openrouter, gateway, free] = &ladders;
    for effort in &shipped_haiku {
        let label = cost_label(vertex, HAIKU, effort);
        assert!(label.ends_with('×'), "{effort}: {label}");
        assert_eq!(cost_label(anthropic, HAIKU, effort), label);
        assert_eq!(cost_label(openrouter, HAIKU, effort), label);
        assert_eq!(
            cost_label(gateway, HAIKU, effort),
            label,
            "a subscription is still paid for"
        );
        assert_eq!(cost_label(free, HAIKU, effort), "included");
        assert_eq!(
            intelligence_label(free, HAIKU, effort),
            intelligence_label(vertex, HAIKU, effort),
            "a route never moves intelligence"
        );
    }
}

#[test]
fn chat_ranks_on_the_general_profile_and_coding_on_the_coding_one() {
    let pick = IntelligenceRung::new(HAIKU, "low");
    let estimate = estimate_for(LadderSurface::Chat, &pick).expect("Haiku low is estimated");
    let chat = served(LadderSurface::Chat, None, "medium", 0.0);
    let reference = estimate_of("claude-opus-5-5", "medium").expect("the reference");
    assert_eq!(
        served_intelligence_rung(&chat, HAIKU, "low").intelligence,
        Band::from_intelligence(estimate.general_band, reference.general_band)
    );
    let coding = served(LadderSurface::ClaudeCode, None, "medium", 0.0);
    assert_eq!(
        served_intelligence_rung(&coding, HAIKU, "low").intelligence,
        Band::from_intelligence(estimate.coding_band, reference.coding_band)
    );
}

/// A rung is estimated when its pair has an estimate, with or without a
/// reference to compare against.
#[test]
fn a_served_intelligence_rung_is_estimated_when_its_pair_has_an_estimate() {
    let override_value = Some("claude-haiku-5-5=low, my-model=high");
    for reference in [Some(opus_at("medium")), None] {
        let ladder = assemble(
            LadderSurface::Chat,
            &OfferAll,
            override_value,
            None,
            reference,
            None,
        );
        assert!(served_intelligence_rung(&ladder, HAIKU, "low").estimated);
        assert!(!served_intelligence_rung(&ladder, "my-model", "high").estimated);
        let on: Vec<(String, bool)> = ladder
            .on_intelligence_rungs()
            .into_iter()
            .map(|r| (r.model, r.estimated))
            .collect();
        assert_eq!(
            on,
            [(HAIKU.to_string(), true), ("my-model".to_string(), false)]
        );
    }
}

#[test]
fn a_chat_override_takes_none_and_drops_an_unoffered_pair() {
    let offered = |model: &str, _: &str| model != "grok-4.6";
    let ladder = effective_ladder(
        LadderSurface::Chat,
        Some("gpt-6-luna=none, grok-4.6=low, claude-opus-5-5[1m]=high"),
        offered,
    );
    assert_eq!(
        pairs_of(&ladder),
        ["gpt-6-luna=none", "claude-opus-5-5[1m]=high"]
    );
    assert!(
        effective_ladder(LadderSurface::ClaudeCode, Some("gpt-6-luna=none"), offered)
            .iter()
            .all(|p| p.effort != "none"),
        "a coding override cannot name none, so it reads as no override"
    );
}

#[test]
fn chat_offers_a_current_model_at_a_tier_one_of_its_routes_takes() {
    let registry = haiku_registry();
    let offer = ChatOffer {
        routes: ModelRoutes {
            registry: registry.clone(),
            configured: ProviderKind::ALL.to_vec(),
        },
        labels: HashMap::new(),
    };
    assert!(offer.offers(HAIKU, "xhigh"), "Anthropic takes xhigh");
    assert!(!offer.offers(HAIKU, "turbo"));
    assert!(!offer.offers("claude-unknown-1", "low"));
    assert_eq!(offer.label(HAIKU), HAIKU, "no label falls back to the id");
    registry
        .write()
        .expect("registry lock")
        .get_mut(HAIKU)
        .expect("Haiku")
        .successor = Some("claude-haiku-6".into());
    assert!(
        !offer.offers(HAIKU, "low"),
        "a retired model is not offered"
    );
}

#[test]
fn a_chat_refusal_names_the_lucidos_agent_ladder_and_its_surface_flag() {
    let ladder = served(LadderSurface::Chat, None, "medium", 0.0);
    let err = ladder
        .check_pick(&IntelligenceRung::new("claude-sonnet-5-5", "low"))
        .unwrap_err();
    assert!(
        err.contains("on the Lucidos Agent intelligence ladder"),
        "{err}"
    );
    assert!(err.contains("--surface chat"), "{err}");
    assert!(ladder
        .check_pick(&IntelligenceRung::new("claude-opus-5-5[1m]", "max"))
        .is_ok());
}

/// The registry the workspace's `models` table seeds, every backend reached.
async fn seeded_routes(pool: &PgPool) -> ModelRoutes {
    let registry = model_registry::empty();
    model_registry::reload(&registry, pool).await;
    ModelRoutes {
        registry,
        configured: ProviderKind::ALL.to_vec(),
    }
}

#[tokio::test]
async fn an_agents_chat_pick_is_bound_to_the_chat_ladder() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let routes = seeded_routes(&pool).await;
    let check = |model: Option<&'static str>, effort: Option<&'static str>| {
        let (pool, routes) = (pool.clone(), routes.clone());
        async move { check_agent_pick(&pool, &routes, LadderSurface::Chat, model, effort).await }
    };
    assert_eq!(check(None, None).await, Ok(()), "no pick is the default");
    assert_eq!(
        check(Some("claude-opus-5-5[1m]"), Some("max")).await,
        Ok(())
    );
    let err = check(Some("claude-sonnet-5-5"), Some("low"))
        .await
        .unwrap_err();
    assert!(
        err.contains("claude-sonnet-5-5=low is not a rung on the Lucidos Agent"),
        "{err}"
    );
    assert!(err.contains("claude-opus-5-5=max"), "{err}");
    let err = check(Some("claude-sonnet-5-5"), None).await.unwrap_err();
    assert!(
        err.contains("claude-sonnet-5-5=high"),
        "a model-only pick runs at its default effort: {err}"
    );
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_chat_ladder_loads_with_the_account_default_marked() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let routes = seeded_routes(&pool).await;
    let ladder = load(&pool, &routes, LadderSurface::Chat).await;
    assert_eq!(ladder.surface, LadderSurface::Chat);
    assert_eq!(ladder.coding_agent, None);
    assert_eq!(ladder.long_prompt_share, None);
    let default = ladder
        .default_intelligence_rung
        .clone()
        .expect("the shipped chat model has a default effort");
    assert_eq!(default.model, prefs::CHAT_MODEL.read(&pool).await);
    assert!(ladder
        .intelligence_rungs
        .iter()
        .any(|r| r.is_default && r.model == default.model && r.effort == default.effort));
    assert!(!ladder.pairs().is_empty());
    crate::test_support::teardown_test_db(&db_name).await;
}
