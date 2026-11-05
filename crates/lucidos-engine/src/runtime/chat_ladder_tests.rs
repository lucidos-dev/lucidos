use super::*;
use crate::llm::model_registry::{self, ProviderKind};
use crate::test_support::{setup_test_db, teardown_test_db};

/// An offer that takes every pair, and joins the models it is given.
struct EveryPair(Vec<IntelligenceRung>);

impl SurfaceOffer for EveryPair {
    fn offers(&self, _: &str, _: &str) -> bool {
        true
    }
    fn label(&self, model: &str) -> String {
        model.to_string()
    }
    fn is_free(&self, _: &str) -> bool {
        false
    }
    fn joining_intelligence_rungs(&self) -> Vec<IntelligenceRung> {
        self.0.clone()
    }
}

fn shipped_on_models() -> Vec<String> {
    let mut models: Vec<String> = LadderSurface::Chat
        .shipped_pairs()
        .iter()
        .filter(|p| p.on)
        .map(|p| p.model.clone())
        .collect();
    models.dedup();
    models
}

fn pair(model: &str, effort: &str) -> IntelligenceRung {
    IntelligenceRung::new(model, effort)
}

#[test]
fn the_shipped_models_on_keep_the_shipped_ladder() {
    let enabled = shipped_on_models();
    let joining = enabled.iter().map(|m| pair(m, "medium")).collect();
    assert_eq!(upgraded_chat_ladder(&EveryPair(joining), &enabled), None);
}

/// An empty override is refused at save. So when no model that was on can
/// join, the upgrade keeps the shipped ladder rather than failing every boot.
#[test]
fn an_upgrade_where_no_enabled_model_joins_keeps_the_shipped_ladder() {
    let enabled = vec!["retired-model".to_string()];
    let joining = vec![pair("claude-haiku-5-5", "low")];
    assert_eq!(upgraded_chat_ladder(&EveryPair(joining), &enabled), None);
}

/// Every model that was on lists, at least on the rung it joins at. A model
/// that was off lists nothing. Shipped order holds, and an unbanded model
/// follows. A model the estimates do not know never lists.
#[test]
fn an_upgrade_lists_every_enabled_model_in_shipped_order() {
    let enabled: Vec<String> = [
        "claude-haiku-5-5",
        "claude-sonnet-5-5[1m]",
        "gpt-5.6-luna",
        "my-model",
    ]
    .map(str::to_string)
    .to_vec();
    let joining = vec![
        pair("claude-haiku-5-5", "low"),
        pair("claude-opus-5-5", "medium"),
        pair("claude-sonnet-5-5", "medium"),
        pair("gpt-5.6-luna", "high"),
        pair("my-model", "high"),
    ];
    let pairs = upgraded_chat_ladder(&EveryPair(joining), &enabled).expect("an override");
    let shipped: Vec<IntelligenceRung> = LadderSurface::Chat
        .shipped_pairs()
        .iter()
        .filter(|p| {
            (p.model == "claude-haiku-5-5" && (p.on || p.effort == "low"))
                || (p.model == "claude-sonnet-5-5" && p.effort == "medium")
        })
        .map(|p| pair(&p.model, &p.effort))
        .collect();
    let mut expected = shipped;
    expected.push(pair("gpt-5.6-luna", "high"));
    assert_eq!(pairs, expected);
    assert!(pairs.iter().all(|p| p.model != "claude-opus-5-5"));
}

/// A rung with no estimate stays off by default: the upgrade lists the
/// estimated model and leaves the unestimated one off.
#[test]
fn an_upgrade_leaves_a_model_without_an_estimate_off() {
    let enabled: Vec<String> = ["claude-haiku-5-5", "my-model"]
        .map(str::to_string)
        .to_vec();
    let joining = vec![pair("claude-haiku-5-5", "low"), pair("my-model", "high")];
    let pairs = upgraded_chat_ladder(&EveryPair(joining), &enabled).expect("an override");
    assert!(pairs.contains(&pair("claude-haiku-5-5", "low")));
    assert!(pairs.iter().all(|p| p.model != "my-model"));
}

/// A model's shipped rungs survive even when the rung it joins at has no
/// estimate: only the joining rung stays off.
#[test]
fn an_upgrade_keeps_the_shipped_rungs_of_a_model_whose_joining_rung_is_unestimated() {
    let enabled: Vec<String> = ["claude-haiku-5-5", "gpt-5.6-luna"]
        .map(str::to_string)
        .to_vec();
    let joining = vec![
        pair("claude-haiku-5-5", "no-such-effort"),
        pair("gpt-5.6-luna", "high"),
    ];
    let pairs = upgraded_chat_ladder(&EveryPair(joining), &enabled).expect("an override");
    assert!(pairs.iter().any(|p| p.model == "claude-haiku-5-5"));
    assert!(!pairs.contains(&pair("claude-haiku-5-5", "no-such-effort")));
}

/// With nothing estimated among the models that were on, the upgrade keeps
/// the shipped ladder rather than saving an empty override.
#[test]
fn an_upgrade_of_only_unestimated_models_keeps_the_shipped_ladder() {
    let enabled = vec!["my-model".to_string()];
    let joining = vec![pair("my-model", "high")];
    assert_eq!(upgraded_chat_ladder(&EveryPair(joining), &enabled), None);
}

async fn routes_of(pool: &PgPool) -> ModelRoutes {
    let registry = model_registry::empty();
    model_registry::reload(&registry, pool).await;
    ModelRoutes {
        registry,
        configured: ProviderKind::ALL.to_vec(),
    }
}

async fn snapshot(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar("SELECT model_id FROM chat_ladder_upgrade_models ORDER BY model_id")
        .fetch_all(pool)
        .await
        .unwrap()
}

const MIGRATION: &str =
    include_str!("../../migrations/20261019131405_chat_ladder_replaces_enabled.sql");

/// The migration seeded with a custom enabled set: every model that was on
/// lists on the chat ladder after, and the upgrade runs once.
#[tokio::test]
async fn upgrading_a_custom_enabled_set_hides_nothing_and_runs_once() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    assert_eq!(
        snapshot(&pool).await,
        Vec::<String>::new(),
        "a new database"
    );

    let on = [
        "claude-haiku-5-5",
        "gemini-3.8-flash",
        "grok-4.20",
        "my-model",
    ];
    for statement in [
        "INSERT INTO models (id, label, routes, sort_order, source) VALUES \
         ('my-model', 'My Model', '[{\"provider\": \"local\"}]', 1000, 'user')"
            .to_string(),
        "UPDATE models SET enabled = FALSE".to_string(),
        format!(
            "UPDATE models SET enabled = TRUE WHERE id IN ({})",
            on.map(|id| format!("'{id}'")).join(", ")
        ),
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload) VALUES \
         (gen_random_uuid(), 'ops', 'global', 'Seeded', '{}')"
            .to_string(),
    ] {
        sqlx::query(&statement)
            .execute(&pool)
            .await
            .expect(&statement);
    }
    sqlx::raw_sql(MIGRATION).execute(&pool).await.unwrap();
    assert_eq!(snapshot(&pool).await, on.map(str::to_string).to_vec());

    let routes = routes_of(&pool).await;
    upgrade_from_enabled(&pool, &bus, &routes).await;
    let ladder = ladder::load(&pool, &routes, LadderSurface::Chat).await;
    assert!(ladder.customised);
    for id in ["claude-haiku-5-5", "gemini-3.8-flash"] {
        assert!(ladder.lists_model(id), "{id} was enabled and must list");
    }
    for id in ["grok-4.20", "my-model"] {
        assert!(
            !ladder.lists_model(id),
            "{id} has no estimate and stays off"
        );
    }
    for id in ["claude-opus-5-5", "gpt-6.1-sol"] {
        assert!(!ladder.lists_model(id), "{id} was off and must stay off");
    }
    assert_eq!(snapshot(&pool).await, Vec::<String>::new());

    PreferenceStore::delete(&pool, &bus, prefs::INTELLIGENCE_LADDER_CHAT.key(), None)
        .await
        .unwrap();
    upgrade_from_enabled(&pool, &bus, &routes).await;
    assert_eq!(
        prefs::INTELLIGENCE_LADDER_CHAT.read(&pool).await,
        None,
        "a reset to the shipped ladder sticks"
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_upgrade_never_overwrites_an_override() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let owned = "claude-haiku-5-5=low";
    PreferenceStore::set(
        &pool,
        &bus,
        prefs::INTELLIGENCE_LADDER_CHAT.key(),
        owned,
        None,
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO chat_ladder_upgrade_models (model_id) VALUES ('grok-4.20')")
        .execute(&pool)
        .await
        .unwrap();
    upgrade_from_enabled(&pool, &bus, &routes_of(&pool).await).await;
    assert_eq!(
        prefs::INTELLIGENCE_LADDER_CHAT.read(&pool).await.as_deref(),
        Some(owned)
    );
    assert_eq!(snapshot(&pool).await, Vec::<String>::new());
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Turning a model on adds the rung it joins at, a repeat changes nothing,
/// and turning it off takes every rung that runs it.
#[tokio::test]
async fn a_model_turns_on_and_off_the_chat_ladder() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let routes = routes_of(&pool).await;
    let set = |model: &'static str, on: bool| {
        let (pool, bus, routes) = (pool.clone(), bus.clone(), routes.clone());
        async move { set_model_on_chat_ladder(&pool, &bus, &routes, model, on, None).await }
    };

    let joining =
        ladder::chat_joining_intelligence_rung(&routes.registry, "grok-4.20").expect("a row");
    assert_eq!(
        set("grok-4.20", true).await,
        Ok(ChatLadderChange::TurnedOn(joining.clone()))
    );
    assert_eq!(
        set("grok-4.20", true).await,
        Ok(ChatLadderChange::Unchanged)
    );
    let ladder = ladder::load(&pool, &routes, LadderSurface::Chat).await;
    assert!(ladder.lists_model("grok-4.20"));
    assert!(
        ladder.lists_model("gpt-6-luna"),
        "the shipped rungs stay on"
    );

    match set("gpt-6-luna", false).await {
        Ok(ChatLadderChange::TurnedOff(off)) => {
            assert!(off.len() > 1 && off.iter().all(|p| p.model == "gpt-6-luna"));
        }
        other => panic!("{other:?}"),
    }
    let ladder = ladder::load(&pool, &routes, LadderSurface::Chat).await;
    assert!(!ladder.lists_model("gpt-6-luna"));
    assert!(ladder.lists_model("grok-4.20"));

    assert!(set("no-such-model", true).await.is_err());
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_last_model_cannot_leave_the_chat_ladder() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    PreferenceStore::set(
        &pool,
        &bus,
        prefs::INTELLIGENCE_LADDER_CHAT.key(),
        "claude-haiku-5-5=low, claude-haiku-5-5=high",
        None,
    )
    .await
    .unwrap();
    let routes = routes_of(&pool).await;
    let err = set_model_on_chat_ladder(&pool, &bus, &routes, "claude-haiku-5-5", false, None)
        .await
        .unwrap_err();
    assert!(err.contains("only model"), "{err}");
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Deleting a user model takes it off the chat ladder, and refuses when it is
/// the last model there, deleting nothing.
#[tokio::test]
async fn deleting_a_user_model_takes_it_off_the_chat_ladder() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    sqlx::query(
        "INSERT INTO models (id, label, routes, sort_order, source) VALUES \
         ('local/my-model', 'My Model', '[{\"provider\": \"local\"}]', 1000, 'user')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let routes = routes_of(&pool).await;
    let mine =
        ladder::chat_joining_intelligence_rung(&routes.registry, "local/my-model").expect("a row");
    PreferenceStore::set(
        &pool,
        &bus,
        prefs::INTELLIGENCE_LADDER_CHAT.key(),
        &override_value(&[mine]),
        None,
    )
    .await
    .unwrap();

    let err = delete_user_model(&pool, &bus, &routes, "local/my-model", None)
        .await
        .unwrap_err();
    assert!(err.contains("only model"), "{err}");
    let kept = crate::core::ModelStore::get(&pool, "local/my-model").await;
    assert!(kept.unwrap().is_some(), "a refusal deletes nothing");

    set_model_on_chat_ladder(&pool, &bus, &routes, "claude-haiku-5-5", true, None)
        .await
        .unwrap();
    assert_eq!(
        delete_user_model(&pool, &bus, &routes, "local/my-model", None).await,
        Ok(true)
    );
    let stored = prefs::INTELLIGENCE_LADDER_CHAT.read(&pool).await.unwrap();
    assert!(!stored.contains("my-model"), "{stored}");
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A model added to the registry joins the served ladder off, unbanded, so
/// the owner can turn it on. A model the shipped pairs name keeps them.
#[tokio::test]
async fn a_user_model_joins_the_chat_ladder_off_and_unbanded() {
    let (pool, db_name) = setup_test_db().await;
    sqlx::query(
        "INSERT INTO models (id, label, routes, sort_order, source) VALUES \
         ('local/my-model', 'My Model', '[{\"provider\": \"local\"}]', 1000, 'user')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let routes = routes_of(&pool).await;
    let ladder = ladder::load(&pool, &routes, LadderSurface::Chat).await;
    let mine: Vec<_> = ladder
        .intelligence_rungs
        .iter()
        .filter(|r| r.model == "local/my-model")
        .collect();
    assert_eq!(mine.len(), 1, "one rung per model");
    assert!(!mine[0].on && mine[0].intelligence.is_none() && mine[0].label == "My Model");
    let luna = ladder
        .intelligence_rungs
        .iter()
        .filter(|r| r.model == "gpt-6-luna")
        .count();
    let shipped_luna = LadderSurface::Chat
        .shipped_pairs()
        .iter()
        .filter(|p| p.model == "gpt-6-luna")
        .count();
    assert_eq!(luna, shipped_luna, "no extra rung for a shipped model");
    assert!(
        ladder
            .intelligence_rungs
            .iter()
            .all(|r| !r.model.ends_with("[1m]")),
        "a [1m] twin is its bare model's rung"
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Copy from Lucidos Agent writes the coding agent's override once, with only
/// the pairs it runs, and leaves the chat override alone. A copy equal to the
/// shipped ladder stores nothing, and chat cannot copy onto itself.
#[tokio::test]
async fn copy_from_chat_writes_the_coding_override_and_leaves_chat_alone() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let chat_value = "claude-haiku-5-5=high, gemini-3.8-flash=high, claude-opus-5-5=max";
    PreferenceStore::set(
        &pool,
        &bus,
        prefs::INTELLIGENCE_LADDER_CHAT.key(),
        chat_value,
        None,
    )
    .await
    .unwrap();
    let routes = routes_of(&pool).await;
    let copy = |target| {
        let (pool, bus, routes) = (pool.clone(), bus.clone(), routes.clone());
        async move { copy_chat_ladder_to(&pool, &bus, &routes, target, None).await }
    };

    let copied = copy(LadderSurface::ClaudeCode).await.expect("a copy");
    let expected = vec![
        pair("claude-haiku-5-5", "high"),
        pair("claude-opus-5-5", "max"),
    ];
    assert_eq!(copied, expected);
    assert_eq!(
        prefs::INTELLIGENCE_LADDER_CLAUDE_CODE
            .read(&pool)
            .await
            .as_deref(),
        Some("claude-haiku-5-5=high, claude-opus-5-5=max")
    );
    assert_eq!(
        prefs::INTELLIGENCE_LADDER_CHAT.read(&pool).await.as_deref(),
        Some(chat_value),
        "the chat override is only read"
    );

    let err = copy(LadderSurface::Codex).await.unwrap_err();
    assert!(err.contains("Codex runs none"), "{err}");
    assert!(prefs::INTELLIGENCE_LADDER_CODEX.read(&pool).await.is_none());
    assert!(copy(LadderSurface::Chat).await.is_err());

    let shipped_cc = ladder::shipped_on(LadderSurface::ClaudeCode, |m, e| {
        ladder::is_offered(crate::runtime::CodingAgent::ClaudeCode, m, e)
    });
    PreferenceStore::set(
        &pool,
        &bus,
        prefs::INTELLIGENCE_LADDER_CHAT.key(),
        &override_value(&shipped_cc),
        None,
    )
    .await
    .unwrap();
    assert_eq!(copy(LadderSurface::ClaudeCode).await, Ok(shipped_cc));
    assert!(
        prefs::INTELLIGENCE_LADDER_CLAUDE_CODE
            .read(&pool)
            .await
            .is_none(),
        "a copy equal to the shipped ladder clears the override"
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Sort by intelligence rewrites the override in the surface's own order and
/// keeps the same rungs on. A sort that lands on the shipped ladder clears the
/// override, so `customised` goes false.
#[tokio::test]
async fn sort_ladder_writes_the_sorted_override_and_clears_one_equal_to_shipped() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let routes = routes_of(&pool).await;
    let target = LadderSurface::ClaudeCode;
    let set = |value: String| {
        let (pool, bus) = (pool.clone(), bus.clone());
        async move {
            PreferenceStore::set(
                &pool,
                &bus,
                prefs::INTELLIGENCE_LADDER_CLAUDE_CODE.key(),
                &value,
                None,
            )
            .await
            .unwrap();
        }
    };

    set("claude-sonnet-5-5=low, claude-haiku-5-5=high".into()).await;
    assert!(!ladder::load(&pool, &routes, target).await.climbs);
    let sorted = sort_ladder(&pool, &bus, &routes, target, None)
        .await
        .expect("a sort");
    assert_eq!(
        sorted,
        vec![
            pair("claude-haiku-5-5", "high"),
            pair("claude-sonnet-5-5", "low")
        ]
    );
    assert_eq!(
        prefs::INTELLIGENCE_LADDER_CLAUDE_CODE
            .read(&pool)
            .await
            .as_deref(),
        Some("claude-haiku-5-5=high, claude-sonnet-5-5=low")
    );
    let served = ladder::load(&pool, &routes, target).await;
    assert!(served.climbs && served.customised);

    let mut swapped = served.shipped_intelligence_rungs.clone();
    swapped.swap(0, 1);
    set(override_value(&swapped)).await;
    assert!(!ladder::load(&pool, &routes, target).await.climbs);
    let sorted = sort_ladder(&pool, &bus, &routes, target, None)
        .await
        .expect("a sort");
    assert_eq!(sorted, served.shipped_intelligence_rungs);
    assert!(prefs::INTELLIGENCE_LADDER_CLAUDE_CODE
        .read(&pool)
        .await
        .is_none());
    assert!(!ladder::load(&pool, &routes, target).await.customised);
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The chat ladder is the only switch for what the picker lists, so no code
/// reads the `models.enabled` column it replaced. The column itself stays
/// until a later migration drops it.
#[test]
fn nothing_reads_the_enabled_column_of_models() {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let statement = regex::Regex::new(r"\b(?:FROM|UPDATE|INTO|JOIN)\s+models\b").unwrap();
    let enabled = regex::Regex::new(r"\benabled\b").unwrap();
    let mut offenders = Vec::new();
    let mut dirs = vec![crates];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if path.is_dir() {
                if name != "target" && name != "node_modules" {
                    dirs.push(path);
                }
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") || name == "chat_ladder_tests.rs" {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for found in statement.find_iter(&text) {
                let start = text[..found.start()].rfind('"').map_or(0, |i| i + 1);
                let end = text[found.end()..]
                    .find('"')
                    .map_or(text.len(), |i| found.end() + i);
                if enabled.is_match(&text[start..end]) {
                    offenders.push(format!("{}: {}", path.display(), &text[start..end]));
                }
            }
        }
    }
    assert!(offenders.is_empty(), "{offenders:#?}");
}
