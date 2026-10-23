//! `retire_model`, the SQL function a migration calls when a provider retires
//! a model (ADR 0418). These tests seed every store it must rewrite, and pin
//! each call's tier list to the Rust ladders it mirrors.

use super::*;
use crate::core::prefs;
use crate::llm::reasoning::{clamp_effort, supported_efforts, EFFORT_LADDER};
use crate::llm::ProviderKind;
use crate::test_support::{setup_test_db, teardown_test_db};

/// One `SELECT retire_model(...)` a migration makes.
#[derive(Debug)]
struct Retirement {
    old: String,
    successor: String,
    accepted: Vec<String>,
}

/// A call starts its line, so the example in a comment is not one.
fn retire_call() -> regex::Regex {
    regex::Regex::new(r"(?m)^SELECT retire_model\('([^']+)',\s*'([^']+)',\s*ARRAY\[([^\]]*)\]\);")
        .unwrap()
}

/// Every migration as (file name, SQL), in migration order.
fn migrations() -> Vec<(String, String)> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/migrations");
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    files.sort();
    files
        .iter()
        .map(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            (name, std::fs::read_to_string(path).unwrap())
        })
        .collect()
}

/// Every retirement any migration declares, in migration order.
fn retirements() -> Vec<Retirement> {
    let call = retire_call();
    migrations()
        .iter()
        .flat_map(|(_, sql)| {
            call.captures_iter(sql)
                .map(|c| Retirement {
                    old: c[1].to_string(),
                    successor: c[2].to_string(),
                    accepted: c[3]
                        .split(',')
                        .map(|tier| tier.trim().trim_matches('\'').to_string())
                        .collect(),
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The retirement in force for each model: a later migration may re-point a
/// model an earlier one retired.
fn latest_retirements() -> Vec<Retirement> {
    let mut latest: Vec<Retirement> = Vec::new();
    for r in retirements() {
        latest.retain(|earlier| earlier.old != r.old);
        latest.push(r);
    }
    latest
}

fn retire_sql(r: &Retirement) -> String {
    let tiers: Vec<String> = r.accepted.iter().map(|t| format!("'{t}'")).collect();
    format!(
        "SELECT retire_model('{}', '{}', ARRAY[{}])",
        r.old,
        r.successor,
        tiers.join(", ")
    )
}

#[test]
fn gemini_3_5_flash_is_retired_to_3_8_flash() {
    let all = retirements();
    let gemini = all
        .iter()
        .find(|r| r.old == "gemini-3.5-flash")
        .expect("a migration retires Gemini 3.5 Flash");
    assert_eq!(gemini.successor, "gemini-3.8-flash");
}

const SEED_SUCCESSORS: &str = include_str!(
    "../../migrations/20261018074150_seed_the_successors_the_retirement_migration_needs.sql"
);
const RETIRE_SUPERSEDED: &str = include_str!(
    "../../migrations/20261018074151_retire_superseded_models_and_seed_their_successors.sql"
);

/// The newest migration whose successors another migration seeds. Later ones
/// seed their own. It and older ones are applied history, and sqlx refuses
/// to boot on an edited applied migration.
const LAST_UNSELF_SEEDED_MIGRATION: &str = "20261018074151";

/// Each successor `sql` retires to before an `INSERT INTO models` in `sql`
/// seeds it.
fn unseeded_successors(sql: &str) -> Vec<String> {
    retire_call()
        .captures_iter(sql)
        .filter(|c| {
            let before = &sql[..c.get(0).unwrap().start()];
            !(before.contains("INSERT INTO models") && before.contains(&format!("('{}',", &c[2])))
        })
        .map(|c| c[2].to_string())
        .collect()
}

#[test]
fn unseeded_successors_finds_a_retirement_with_no_seed_before_it() {
    let retire = "SELECT retire_model('old', 'new', ARRAY['low']);\n";
    let seed = "INSERT INTO models (id) VALUES\n  ('new', 'New')\nON CONFLICT (id) DO NOTHING;\n";
    assert_eq!(unseeded_successors(retire), ["new"]);
    assert_eq!(unseeded_successors(&format!("{retire}{seed}")), ["new"]);
    assert!(unseeded_successors(&format!("{seed}{retire}")).is_empty());
}

/// `retire_model` raises on a missing successor, and a raise in a migration
/// stops the engine at boot. A workspace can lack a row an older migration
/// seeded, so each migration seeds every successor before retiring to it.
#[test]
fn every_migration_seeds_each_successor_before_retiring_to_it() {
    for (name, sql) in migrations() {
        let version = &name[..LAST_UNSELF_SEEDED_MIGRATION.len()];
        if version > LAST_UNSELF_SEEDED_MIGRATION {
            assert_eq!(unseeded_successors(&sql), Vec::<String>::new(), "{name}");
        }
    }
    let applied_in_order = format!("{SEED_SUCCESSORS}{RETIRE_SUPERSEDED}");
    assert_eq!(unseeded_successors(&applied_in_order), Vec::<String>::new());
}

/// A workspace that lost successor rows still migrates, and gets back the
/// exact rows the earlier seeds and route migrations produced.
#[tokio::test]
async fn the_retirement_migration_restores_successors_the_workspace_lost() {
    let (pool, db_name) = setup_test_db().await;
    let mut successors: Vec<String> = retirements()
        .into_iter()
        .filter(|r| RETIRE_SUPERSEDED.contains(&retire_sql(r)))
        .map(|r| r.successor)
        .collect();
    successors.sort();
    successors.dedup();
    assert!(successors.contains(&"claude-fable-5-1".to_string()));

    let snapshot = |pool: PgPool, ids: Vec<String>| async move {
        sqlx::query_as::<
            _,
            (
                String,
                String,
                serde_json::Value,
                i32,
                String,
                bool,
                Option<String>,
            ),
        >(
            "SELECT id, label, routes, sort_order, source, vision, default_effort \
             FROM models WHERE id = ANY($1) ORDER BY id",
        )
        .bind(ids)
        .fetch_all(&pool)
        .await
        .unwrap()
    };
    let before = snapshot(pool.clone(), successors.clone()).await;
    assert_eq!(before.len(), successors.len(), "every successor has a row");

    sqlx::query("DELETE FROM models WHERE id = ANY($1)")
        .bind(&successors)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(SEED_SUCCESSORS).execute(&pool).await.unwrap();
    sqlx::raw_sql(RETIRE_SUPERSEDED)
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(snapshot(pool.clone(), successors.clone()).await, before);
    let fable = ModelStore::get(&pool, "claude-fable-5")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fable.successor.as_deref(), Some("claude-fable-5-1"));

    // A database that applied the retirement first runs the seed after it.
    sqlx::raw_sql(SEED_SUCCESSORS).execute(&pool).await.unwrap();
    assert_eq!(snapshot(pool.clone(), successors).await, before);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn every_superseded_model_is_retired_to_the_audited_successor() {
    let all = latest_retirements();
    for (old, successor) in [
        ("claude-opus-5", "claude-opus-5-5"),
        ("claude-opus-5[1m]", "claude-opus-5-5"),
        ("claude-sonnet-5", "claude-sonnet-5-5"),
        ("claude-sonnet-5[1m]", "claude-sonnet-5-5"),
        ("claude-fable-5", "claude-fable-5-1"),
        ("claude-fable-5[1m]", "claude-fable-5-1"),
        ("claude-opus-4-8[1m]", "claude-opus-5-5"),
        ("claude-opus-4-7[1m]", "claude-opus-5-5"),
        ("claude-opus-4-6[1m]", "claude-opus-5-5"),
        ("claude-sonnet-4-6[1m]", "claude-sonnet-5-5"),
        ("claude-haiku-4-5", "claude-haiku-5-5"),
        ("gemini-3-flash-preview", "gemini-3.8-flash"),
        ("gpt-5.4-mini", "gpt-6-luna"),
        ("gpt-5.6-luna", "gpt-6-luna"),
        ("gpt-5.6-sol", "gpt-6.1-sol"),
        ("gpt-5.6-terra", "gpt-6.1-sol"),
        ("gpt-5.5-pro", "gpt-6-astra"),
        ("claude-fable-5-1[1m]", "claude-fable-5-1"),
        ("claude-opus-5-5[1m]", "claude-opus-5-5"),
        ("claude-sonnet-5-5[1m]", "claude-sonnet-5-5"),
    ] {
        let r = all
            .iter()
            .find(|r| r.old == old)
            .unwrap_or_else(|| panic!("no migration retires {old}"));
        assert_eq!(r.successor, successor, "{old}");
    }
}

/// The tier list a migration passes must be exactly what EVERY route of the
/// successor accepts, so a snapped effort is one each route takes. Where all
/// routes agree, the SQL snap must also land where `clamp_effort` does.
#[tokio::test]
async fn each_retirement_snaps_efforts_the_way_the_router_does() {
    let (pool, db_name) = setup_test_db().await;
    for r in latest_retirements() {
        let successor = ModelStore::get(&pool, &r.successor)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("successor {} has no row", r.successor));
        assert!(
            successor.successor.is_none(),
            "{} retires to {}, which is itself retired",
            r.old,
            r.successor
        );
        let route_tiers: Vec<(ProviderKind, &str, &[&str])> = successor
            .routes
            .iter()
            .map(|route| {
                let provider = ProviderKind::parse(&route.provider);
                let wire_id = route.wire_id(&successor.id);
                (provider, wire_id, supported_efforts(provider, wire_id))
            })
            .collect();
        let every_route_takes: Vec<&str> = EFFORT_LADDER
            .iter()
            .copied()
            .filter(|tier| route_tiers.iter().all(|(_, _, tiers)| tiers.contains(tier)))
            .collect();
        assert_eq!(
            r.accepted, every_route_takes,
            "{}: the migration's tiers differ from what every route accepts",
            r.successor
        );
        for tier in EFFORT_LADDER {
            let snapped: Option<String> = sqlx::query_scalar("SELECT snap_effort($1, $2)")
                .bind(tier)
                .bind(&r.accepted)
                .fetch_one(&pool)
                .await
                .unwrap();
            for (provider, wire_id, tiers) in &route_tiers {
                if *tiers == r.accepted.as_slice() {
                    assert_eq!(
                        snapped.as_deref(),
                        clamp_effort(tier, *provider, wire_id),
                        "{tier} on {wire_id}"
                    );
                } else {
                    let snapped = snapped.as_deref().unwrap();
                    assert!(
                        tiers.contains(&snapped),
                        "{tier} snaps to {snapped} on {wire_id}"
                    );
                }
            }
        }

        let old = ModelStore::get(&pool, &r.old).await.unwrap();
        if let Some(old) = old {
            assert_eq!(old.successor.as_deref(), Some(r.successor.as_str()));
        }
    }
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The startup fallback list must not offer a model the registry retired.
#[test]
fn the_frontend_model_list_offers_no_retired_model() {
    let models_ts = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../lucidos-app/src/store/models.ts"
    ))
    .unwrap();
    for r in retirements() {
        assert!(
            !models_ts.contains(&format!("value: '{}'", r.old)),
            "store/models.ts still offers the retired {}",
            r.old
        );
    }
}

/// A coding agent's settings name its own lineup, which still offers ids the
/// registry retires, so none of them moves. A list already naming the
/// successor keeps the user's own entry and drops the retired one.
#[tokio::test]
async fn retire_model_leaves_coding_agents_alone_and_never_duplicates_a_pair() {
    let (pool, db_name) = setup_test_db().await;
    let all = retirements();
    let retire = |old: &str| {
        all.iter()
            .find(|r| r.old == old)
            .map(retire_sql)
            .unwrap_or_else(|| panic!("no migration retires {old}"))
    };
    let (chat_thread, agent_thread) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
    let coding_agent_settings = [
        (prefs::MODEL_CLAUDE_CODE.key(), "claude-opus-5"),
        (prefs::REASONING_CLAUDE_CODE.key(), "max"),
        (prefs::MODEL_CODEX.key(), "gpt-5.6-sol"),
        (prefs::REASONING_CODEX.key(), "max"),
        (
            prefs::INTELLIGENCE_LADDER_CLAUDE_CODE.key(),
            "claude-sonnet-5=high, claude-opus-5=max",
        ),
        (
            prefs::INTELLIGENCE_LADDER_CODEX.key(),
            "gpt-5.6-luna=medium, gpt-5.6-sol=max",
        ),
    ];
    let efforts = prefs::CHAT_REASONING_EFFORTS.key();
    let values: Vec<String> = coding_agent_settings
        .iter()
        .map(|(key, value)| format!("('{key}', '{value}', NULL)"))
        .collect();
    let seed = [
        "DELETE FROM preferences".to_string(),
        format!(
            "INSERT INTO preferences (key, value, device_id) VALUES {}, \
             ('{efforts}', 'gpt-6.1-sol=low, gpt-5.6-sol=max, gpt-5.6-terra=medium', NULL)",
            values.join(", ")
        ),
        format!(
            "INSERT INTO thread_summaries (thread_id, is_coding_agent, compose_selection) VALUES \
             ('{agent_thread}', TRUE, '{{\"model\": \"claude-opus-5\"}}'), \
             ('{chat_thread}', FALSE, '{{\"model\": \"claude-opus-5\"}}')"
        ),
        format!(
            "INSERT INTO thread_queue (id, kind, summary, request) VALUES \
             ('{agent_thread}', 'coding-agent', 's', \
              '{{\"type\": \"coding-agent\", \"model\": \"gpt-5.6-sol\", \
                \"reasoning_effort\": \"max\"}}')"
        ),
        format!(
            "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload) VALUES \
             (gen_random_uuid(), 'thread', '{agent_thread}', 'MessageReceived', \
              '{{\"model\": \"claude-opus-5\"}}'), \
             (gen_random_uuid(), 'thread', '{chat_thread}', 'MessageReceived', \
              '{{\"model\": \"claude-opus-5\"}}')"
        ),
    ];
    for statement in seed {
        sqlx::query(&statement)
            .execute(&pool)
            .await
            .expect(&statement);
    }
    for old in [
        "claude-opus-5",
        "claude-sonnet-5",
        "gpt-5.6-luna",
        "gpt-5.6-sol",
        "gpt-5.6-terra",
    ] {
        sqlx::query(&retire(old)).execute(&pool).await.unwrap();
    }

    let text = |sql: String| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, Option<String>>(&sql)
                .fetch_one(&pool)
                .await
                .expect(&sql)
        }
    };
    let pref = |key: &str| format!("SELECT value FROM preferences WHERE key = '{key}'");
    for (key, value) in coding_agent_settings {
        assert_eq!(text(pref(key)).await.as_deref(), Some(value), "{key}");
    }
    assert_eq!(
        text(pref(efforts)).await.as_deref(),
        Some("gpt-6.1-sol=low")
    );
    let draft = |thread: uuid::Uuid| {
        format!("SELECT compose_selection ->> 'model' FROM thread_summaries WHERE thread_id = '{thread}'")
    };
    assert_eq!(
        text(draft(agent_thread)).await.as_deref(),
        Some("claude-opus-5")
    );
    assert_eq!(
        text(draft(chat_thread)).await.as_deref(),
        Some("claude-opus-5-5")
    );
    assert_eq!(
        text(format!(
            "SELECT request ->> 'model' FROM thread_queue WHERE id = '{agent_thread}'"
        ))
        .await
        .as_deref(),
        Some("gpt-5.6-sol")
    );
    let turn = |thread: uuid::Uuid| {
        format!("SELECT payload ->> 'model' FROM events WHERE aggregate_id = '{thread}'")
    };
    assert_eq!(
        text(turn(agent_thread)).await.as_deref(),
        Some("claude-opus-5")
    );
    assert_eq!(
        text(turn(chat_thread)).await.as_deref(),
        Some("claude-opus-5-5")
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Every live setting moves to the successor at an effort it takes. Every
/// record of what happened, and every id that only starts like the retired
/// one, stays as it was. A second run changes nothing.
#[tokio::test]
async fn retire_model_moves_every_live_setting_and_leaves_history() {
    let (pool, db_name) = setup_test_db().await;
    let gemini = retirements()
        .into_iter()
        .find(|r| r.old == "gemini-3.5-flash")
        .unwrap();
    let thread = uuid::Uuid::new_v4();
    let (chat, trigger, memory, title, judge) = (
        prefs::CHAT_MODEL.key(),
        prefs::TRIGGER_MODEL.key(),
        prefs::MODEL_MEMORY.key(),
        prefs::MODEL_TITLE.key(),
        prefs::MODEL_COMMAND_JUDGE.key(),
    );
    let (memory_tier, title_tier, judge_tier) = (
        prefs::REASONING_MEMORY.key(),
        prefs::REASONING_TITLE.key(),
        prefs::REASONING_COMMAND_JUDGE.key(),
    );
    let (efforts, caps) = (
        prefs::CHAT_REASONING_EFFORTS.key(),
        prefs::MEMORY_VIEW_MODEL_CAPS.key(),
    );
    let seed = [
        "DELETE FROM preferences".to_string(),
        format!(
            "INSERT INTO preferences (key, value, device_id) VALUES \
             ('{chat}', 'gemini-3.5-flash', NULL), \
             ('{trigger}', 'gemini-3.5-flash', NULL), \
             ('{memory}', 'gemini-3.5-flash', NULL), \
             ('{memory_tier}', 'none', NULL), \
             ('{title}', 'gemini-3.5-flash', 'device-1'), \
             ('{title_tier}', 'max', NULL), \
             ('{judge}', 'gemini-3.5-flash-lite', NULL), \
             ('{judge_tier}', 'none', NULL), \
             ('a_note', 'gemini-3.5-flash', NULL), \
             ('{efforts}', \
              'claude-opus-5-5=high, gemini-3.5-flash=none,gemini-3.5-flash-lite=none', NULL), \
             ('{caps}', 'gemini-3.5-flash=40000', NULL)"
        ),
        format!(
            "INSERT INTO thread_summaries (thread_id, compose_selection) VALUES \
             ('{thread}', '{{\"model\": \"gemini-3.5-flash\", \"reasoningEffort\": \"none\", \
               \"provider\": \"vertex\"}}')"
        ),
        format!(
            "INSERT INTO thread_queue (id, kind, summary, request) VALUES \
             ('{thread}', 'sub-thread', 's', \
              '{{\"type\": \"sub-thread\", \"model\": \"gemini-3.5-flash\", \
                \"reasoning_effort\": \"xhigh\"}}')"
        ),
        format!(
            "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload) VALUES \
             (gen_random_uuid(), 'thread', '{thread}', 'MessageReceived', \
              '{{\"model\": \"gemini-3.5-flash\", \"reasoning_effort\": \"none\"}}'), \
             (gen_random_uuid(), 'thread', '{thread}', 'TriggerStarted', \
              '{{\"model\": \"gemini-3.5-flash\"}}'), \
             (gen_random_uuid(), 'trigger', 't1', 'TriggerCreated', \
              '{{\"id\": \"t1\", \"model\": \"gemini-3.5-flash\", \"reasoning_effort\": \"max\"}}'), \
             (gen_random_uuid(), 'trigger', 't1', 'TriggerUpdated', \
              '{{\"id\": \"t1\", \"model\": \"gemini-3.5-flash\", \"reasoning_effort\": null}}'), \
             (gen_random_uuid(), 'thread', '{thread}', 'ResponseGenerated', \
              '{{\"model\": \"gemini-3.5-flash\"}}'), \
             (gen_random_uuid(), 'thread', '{thread}', 'ContextCaptured', \
              '{{\"model\": \"gemini-3.5-flash\"}}')"
        ),
    ];
    for statement in seed {
        sqlx::query(&statement)
            .execute(&pool)
            .await
            .expect(&statement);
    }

    let snapshot = || {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT concat_ws(' | ', \
                   (SELECT string_agg(key || coalesce('@' || device_id, '') || '=' || value, \
                                      '; ' ORDER BY key, device_id) FROM preferences), \
                   (SELECT string_agg(compose_selection::text, '; ') FROM thread_summaries \
                     WHERE compose_selection IS NOT NULL), \
                   (SELECT string_agg(request::text, '; ') FROM thread_queue), \
                   (SELECT string_agg(event_type || payload::text, '; ' ORDER BY event_type) \
                      FROM events WHERE payload ? 'model'))",
            )
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    sqlx::query(&retire_sql(&gemini))
        .execute(&pool)
        .await
        .unwrap();
    let first = snapshot().await;
    sqlx::query(&retire_sql(&gemini))
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(snapshot().await, first, "a second run changes nothing");

    let text = |sql: String| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, Option<String>>(&sql)
                .fetch_one(&pool)
                .await
                .expect(&sql)
        }
    };
    let pref = |key: &str| format!("SELECT value FROM preferences WHERE key = '{key}'");
    for key in [chat, trigger, memory, title] {
        assert_eq!(
            text(pref(key)).await.as_deref(),
            Some("gemini-3.8-flash"),
            "{key}"
        );
    }
    assert_eq!(text(pref(memory_tier)).await.as_deref(), Some("low"));
    assert_eq!(text(pref(title_tier)).await.as_deref(), Some("high"));
    assert_eq!(
        text(pref(efforts)).await.as_deref(),
        Some("claude-opus-5-5=high, gemini-3.8-flash=low,gemini-3.5-flash-lite=none")
    );
    assert_eq!(
        text(pref(caps)).await.as_deref(),
        Some("gemini-3.8-flash=40000")
    );
    // A value that is not a model setting is not rewritten.
    assert_eq!(
        text(pref("a_note")).await.as_deref(),
        Some("gemini-3.5-flash")
    );
    // An id that only starts like the retired one is another model.
    assert_eq!(
        text(pref(judge)).await.as_deref(),
        Some("gemini-3.5-flash-lite")
    );
    assert_eq!(text(pref(judge_tier)).await.as_deref(), Some("none"));

    assert_eq!(
        text(format!(
            "SELECT compose_selection::text FROM thread_summaries WHERE thread_id = '{thread}'"
        ))
        .await
        .as_deref(),
        Some(r#"{"model": "gemini-3.8-flash", "provider": "vertex", "reasoningEffort": "low"}"#)
    );
    assert_eq!(
        text(format!(
            "SELECT request::text FROM thread_queue WHERE id = '{thread}'"
        ))
        .await
        .as_deref(),
        Some(r#"{"type": "sub-thread", "model": "gemini-3.8-flash", "reasoning_effort": "high"}"#)
    );

    let event = |event_type: &str| {
        format!("SELECT payload::text FROM events WHERE event_type = '{event_type}'")
    };
    for (event_type, payload) in [
        (
            "MessageReceived",
            r#"{"model": "gemini-3.8-flash", "reasoning_effort": "low"}"#,
        ),
        ("TriggerStarted", r#"{"model": "gemini-3.8-flash"}"#),
        (
            "TriggerCreated",
            r#"{"id": "t1", "model": "gemini-3.8-flash", "reasoning_effort": "high"}"#,
        ),
        (
            "TriggerUpdated",
            r#"{"id": "t1", "model": "gemini-3.8-flash", "reasoning_effort": null}"#,
        ),
        // History: these record which model answered and what it cost.
        ("ResponseGenerated", r#"{"model": "gemini-3.5-flash"}"#),
        ("ContextCaptured", r#"{"model": "gemini-3.5-flash"}"#),
    ] {
        assert_eq!(
            text(event(event_type)).await.as_deref(),
            Some(payload),
            "{event_type}"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The chat ladder moves each retired rung to the successor at a tier it
/// takes, beside the successor's own rungs. A rung it would duplicate is
/// dropped, so the override still parses.
#[tokio::test]
async fn retire_model_moves_chat_ladder_rungs_without_duplicating_one() {
    let (pool, db_name) = setup_test_db().await;
    let all = retirements();
    let key = prefs::INTELLIGENCE_LADDER_CHAT.key();
    sqlx::query("INSERT INTO preferences (key, value) VALUES ($1, $2)")
        .bind(key)
        .bind(
            "claude-opus-5-5=high, claude-opus-5=medium, claude-opus-5=xhigh, \
             claude-opus-5-5=xhigh, gpt-5.6-sol=xhigh, gpt-5.6-sol=max",
        )
        .execute(&pool)
        .await
        .unwrap();
    for old in ["claude-opus-5", "gpt-5.6-sol"] {
        let retirement = all.iter().find(|r| r.old == old).unwrap();
        sqlx::query(&retire_sql(retirement))
            .execute(&pool)
            .await
            .unwrap();
    }
    let value: String = sqlx::query_scalar("SELECT value FROM preferences WHERE key = $1")
        .bind(key)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        value,
        "claude-opus-5-5=high, claude-opus-5-5=medium, claude-opus-5-5=xhigh, gpt-6.1-sol=high"
    );
    assert!(
        prefs::parse_intelligence_ladder(&value, prefs::REASONING_EFFORTS).is_ok(),
        "{value}"
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

const RETIRE_TWINS: &str =
    include_str!("../../migrations/20261019180742_retire_the_redundant_1m_twins.sql");

/// The twins of the models that only run at 1M retire to their bare rows.
/// Every bare route then declares 1M, a route's own window survives, and a
/// cost row naming a twin learns the id the provider answered with.
#[tokio::test]
async fn the_1m_twins_retire_to_bare_rows_that_keep_a_1m_window() {
    let (pool, db_name) = setup_test_db().await;
    let bare = ["claude-fable-5-1", "claude-opus-5-5", "claude-sonnet-5-5"];

    // The state a live workspace had: bare Fable 5.1 declared no window, and
    // one Opus 5.5 route carries a window of its own.
    sqlx::raw_sql(
        "UPDATE models SET routes = '[{\"provider\": \"anthropic\"}, \
           {\"provider\": \"claude-gateway\"}]'::jsonb WHERE id = 'claude-fable-5-1'; \
         UPDATE models SET routes = '[{\"provider\": \"vertex\", \"context_window\": 500000}, \
           {\"provider\": \"anthropic\"}]'::jsonb WHERE id = 'claude-opus-5-5'; \
         INSERT INTO events (id, aggregate, aggregate_id, event_type, payload) VALUES \
           (gen_random_uuid(), 'thread', 'a', 'ContextCaptured', \
            '{\"model\": \"claude-opus-5-5[1m]\"}'), \
           (gen_random_uuid(), 'thread', 'b', 'ContextCaptured', \
            '{\"model\": \"claude-opus-5-5[1m]\", \"served_model\": \"claude-opus-5\"}'), \
           (gen_random_uuid(), 'thread', 'c', 'ContextCaptured', \
            '{\"model\": \"claude-opus-5-5\"}');",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::raw_sql(RETIRE_TWINS).execute(&pool).await.unwrap();

    for id in bare {
        let twin = ModelStore::get(&pool, &format!("{id}[1m]"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(twin.successor.as_deref(), Some(id));
        let row = ModelStore::get(&pool, id).await.unwrap().unwrap();
        assert!(row.successor.is_none(), "{id} is current");
        for route in &row.routes {
            let expected = if id == "claude-opus-5-5" && route.provider == "vertex" {
                500_000
            } else {
                1_000_000
            };
            assert_eq!(
                route.context_window,
                Some(expected),
                "{id} on {}",
                route.provider
            );
        }
    }

    let captures: Vec<String> = sqlx::query_scalar(
        "SELECT payload::text FROM events WHERE event_type = 'ContextCaptured' \
          ORDER BY aggregate_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        captures,
        [
            r#"{"model": "claude-opus-5-5[1m]", "served_model": "claude-opus-5-5"}"#,
            r#"{"model": "claude-opus-5-5[1m]", "served_model": "claude-opus-5"}"#,
            r#"{"model": "claude-opus-5-5"}"#,
        ]
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}
