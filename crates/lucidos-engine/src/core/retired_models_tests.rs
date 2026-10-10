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

/// Every retirement any migration declares, in migration order. A call starts
/// its line, so the example in a comment is not one.
fn retirements() -> Vec<Retirement> {
    let call = regex::Regex::new(
        r"(?m)^SELECT retire_model\('([^']+)',\s*'([^']+)',\s*ARRAY\[([^\]]*)\]\);",
    )
    .unwrap();
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/migrations");
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    files.sort();
    files
        .iter()
        .flat_map(|path| {
            let sql = std::fs::read_to_string(path).unwrap();
            call.captures_iter(&sql)
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

/// The tier list a migration passes must be exactly what every route of the
/// successor accepts, and the SQL snap must land where `clamp_effort` does.
/// A drift here writes an effort the successor then refuses.
#[tokio::test]
async fn each_retirement_snaps_efforts_the_way_the_router_does() {
    let (pool, db_name) = setup_test_db().await;
    for r in retirements() {
        let successor = ModelStore::get(&pool, &r.successor)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("successor {} has no row", r.successor));
        for route in &successor.routes {
            let provider = ProviderKind::parse(&route.provider);
            let wire_id = route.wire_id(&successor.id);
            assert_eq!(
                r.accepted,
                supported_efforts(provider, wire_id),
                "{} on {}: the migration's tiers differ from the route's",
                r.successor,
                route.provider
            );
            for tier in EFFORT_LADDER {
                let snapped: Option<String> = sqlx::query_scalar("SELECT snap_effort($1, $2)")
                    .bind(tier)
                    .bind(&r.accepted)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
                assert_eq!(
                    snapped.as_deref(),
                    clamp_effort(tier, provider, wire_id),
                    "{tier} on {wire_id}"
                );
            }
        }

        let old = ModelStore::get(&pool, &r.old).await.unwrap();
        if let Some(old) = old {
            assert!(!old.enabled, "{} stays offered", r.old);
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
    let (chat, memory, title, judge) = (
        prefs::CHAT_MODEL.key(),
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
    for key in [chat, memory, title] {
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
