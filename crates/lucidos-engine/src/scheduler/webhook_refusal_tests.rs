use super::*;
use crate::core::webhooks::{DeliveryRefusal, RefusalRun, WebhookConfig, WebhookStore};
use crate::engine::event_bus::EventBus;
use std::collections::BTreeMap;
use uuid::Uuid;

const RUN_SECS: i64 = 90_000;

fn ago(secs: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now() - chrono::Duration::seconds(secs)
}

fn hook(id: Uuid, name: &str, enabled: bool, reasons: &[(DeliveryRefusal, i64)]) -> Webhook {
    let refusals: i64 = reasons.iter().map(|(_, n)| *n).sum();
    let running = refusals > 0;
    Webhook {
        id,
        name: name.into(),
        event_type: "GithubWorkflowRunStateChanged".into(),
        token_hash: Some("x".into()),
        hmac: None,
        dedupe: None,
        headers: Vec::new(),
        enabled,
        created_at: ago(900_000),
        updated_at: ago(900_000),
        last_accepted_at: None,
        last_refused_at: running.then(|| ago(120)),
        last_refusal_reason: reasons.first().map(|(r, _)| r.reason().to_string()),
        refusal_run: RefusalRun {
            refusals,
            since: running.then(|| ago(RUN_SECS)),
            cause: reasons.first().map(|(r, _)| r.cause()),
            reasons: reasons
                .iter()
                .map(|(r, n)| (r.key().to_string(), *n))
                .collect(),
            run_secs: running.then_some(RUN_SECS),
            quiet_secs: running.then_some(120),
        },
    }
}

fn accepting(id: Uuid, name: &str) -> Webhook {
    Webhook {
        last_accepted_at: Some(ago(30)),
        ..hook(id, name, true, &[])
    }
}

fn standing(name: &str, cause: RefusalCause) -> StandingRefusal {
    StandingRefusal {
        webhook_name: name.into(),
        declared: Declared {
            cause,
            since: ago(RUN_SECS),
        },
        refusing_secs: RUN_SECS,
    }
}

fn nothing_declared() -> HashMap<String, StandingRefusal> {
    HashMap::new()
}

#[test]
fn a_workspace_whose_only_hook_is_switched_off_is_still_judged() {
    let id = Uuid::new_v4();
    let hooks = vec![hook(
        id,
        "GitHub workflow runs",
        false,
        &[(DeliveryRefusal::Disabled, 42)],
    )];

    let planned = plan(&hooks, &nothing_declared());
    let [SystemEvent::WebhookDeliveriesRefused {
        webhook_id,
        webhook_name,
        enabled,
        cause,
        refusals,
        reasons,
        refusing_secs,
        ..
    }] = planned.as_slice()
    else {
        panic!("expected one declaration, got {planned:?}");
    };
    assert_eq!(webhook_id, &id.to_string());
    assert_eq!(webhook_name, "GitHub workflow runs");
    assert!(
        !enabled,
        "the recovery is one click, so the page needs this"
    );
    assert_eq!(*cause, RefusalCause::Disabled);
    assert_eq!(*refusals, 42);
    assert_eq!(reasons.get("disabled"), Some(&42));
    assert_eq!(
        *refusing_secs, RUN_SECS,
        "the age is the one the database measured, not a host reading"
    );
}

#[test]
fn a_standing_declaration_is_announced_once() {
    let id = Uuid::new_v4();
    let hooks = vec![hook(
        id,
        "deploys",
        false,
        &[(DeliveryRefusal::Disabled, 9)],
    )];
    let declared = HashMap::from([(id.to_string(), standing("deploys", RefusalCause::Disabled))]);
    assert!(plan(&hooks, &declared).is_empty());
}

#[test]
fn one_isolated_refusal_announces_nothing() {
    let id = Uuid::new_v4();
    let hooks = vec![hook(
        id,
        "deploys",
        true,
        &[(DeliveryRefusal::SignatureMismatch, 1)],
    )];
    assert!(plan(&hooks, &nothing_declared()).is_empty());
}

#[test]
fn an_acceptance_retracts_the_declaration() {
    let id = Uuid::new_v4();
    let declared = HashMap::from([(
        id.to_string(),
        standing("deploys", RefusalCause::Verification),
    )]);

    let planned = plan(&[accepting(id, "deploys")], &declared);
    let [SystemEvent::WebhookDeliveriesRecovered {
        webhook_id,
        cause,
        resolution,
        refusing_secs,
        ..
    }] = planned.as_slice()
    else {
        panic!("expected one retraction, got {planned:?}");
    };
    assert_eq!(webhook_id, &id.to_string());
    assert_eq!(*cause, RefusalCause::Verification);
    assert_eq!(*resolution, Resolution::Accepted);
    assert_eq!(*refusing_secs, RUN_SECS - 30);
}

#[test]
fn a_declaration_whose_hook_is_gone_is_retracted() {
    let gone = Uuid::new_v4();
    let declared = HashMap::from([(
        gone.to_string(),
        standing("deploys", RefusalCause::Disabled),
    )]);

    let planned = plan(&[], &declared);
    let [SystemEvent::WebhookDeliveriesRecovered {
        webhook_id,
        webhook_name,
        resolution,
        ..
    }] = planned.as_slice()
    else {
        panic!("expected one retraction, got {planned:?}");
    };
    assert_eq!(webhook_id, &gone.to_string());
    assert_eq!(
        webhook_name, "deploys",
        "the name comes off the declaration, since the row is gone"
    );
    assert_eq!(*resolution, Resolution::Removed);
}

#[test]
fn each_hook_is_judged_on_its_own_record() {
    let broken = Uuid::new_v4();
    let fixed = Uuid::new_v4();
    let hooks = vec![
        hook(broken, "github", false, &[(DeliveryRefusal::Disabled, 12)]),
        accepting(fixed, "stripe"),
    ];
    let declared = HashMap::from([
        (
            broken.to_string(),
            standing("github", RefusalCause::Disabled),
        ),
        (
            fixed.to_string(),
            standing("stripe", RefusalCause::Verification),
        ),
    ]);

    let planned = plan(&hooks, &declared);
    assert_eq!(planned.len(), 1, "only the fixed one moved: {planned:?}");
    match &planned[0] {
        SystemEvent::WebhookDeliveriesRecovered { webhook_id, .. } => {
            assert_eq!(webhook_id, &fixed.to_string());
        }
        other => panic!("expected a retraction, got {other:?}"),
    }
}

#[test]
fn a_cycle_announcing_several_orphans_orders_them() {
    let ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();
    let declared: HashMap<String, StandingRefusal> = ids
        .iter()
        .map(|id| (id.to_string(), standing("gone", RefusalCause::Disabled)))
        .collect();

    let announced: Vec<String> = plan(&[], &declared)
        .iter()
        .map(|event| match event {
            SystemEvent::WebhookDeliveriesRecovered { webhook_id, .. } => webhook_id.clone(),
            other => panic!("expected a retraction, got {other:?}"),
        })
        .collect();
    let mut sorted = announced.clone();
    sorted.sort();
    assert_eq!(announced, sorted);
}

fn refused_event(id: Uuid, name: &str, cause: RefusalCause, secs: i64) -> SystemEvent {
    SystemEvent::WebhookDeliveriesRefused {
        webhook_id: id.to_string(),
        webhook_name: name.into(),
        enabled: false,
        cause,
        refusals: 7,
        reasons: BTreeMap::from([("disabled".to_string(), 7)]),
        refusing_since: ago(secs).to_rfc3339(),
        refusing_secs: secs,
    }
}

async fn seed_hook(pool: &sqlx::PgPool, bus: &EventBus, name: &str) -> Webhook {
    WebhookStore::create(
        pool,
        bus,
        name,
        "GithubWorkflowRunStateChanged",
        WebhookConfig::default(),
        None,
    )
    .await
    .unwrap()
    .0
}

#[tokio::test]
async fn the_newest_event_per_hook_decides_what_stands() {
    let (pool, db) = crate::test_support::setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let refusing = seed_hook(&pool, &bus, "github").await;
    let recovered = seed_hook(&pool, &bus, "stripe").await;

    for event in [
        refused_event(refusing.id, "github", RefusalCause::Disabled, RUN_SECS),
        refused_event(recovered.id, "stripe", RefusalCause::Verification, RUN_SECS),
    ] {
        bus.emit(BusEvent::System(event)).await.unwrap();
    }
    bus.emit(BusEvent::System(SystemEvent::WebhookDeliveriesRecovered {
        webhook_id: recovered.id.to_string(),
        webhook_name: "stripe".into(),
        cause: RefusalCause::Verification,
        resolution: Resolution::Accepted,
        refusing_since: ago(RUN_SECS).to_rfc3339(),
        refusing_secs: RUN_SECS,
    }))
    .await
    .unwrap();

    let standing = declared_refusals(&pool).await.unwrap();
    assert_eq!(
        standing.keys().collect::<Vec<_>>(),
        vec![&refusing.id.to_string()],
        "a retraction declares nothing, and the window is per hook"
    );
    let github = &standing[&refusing.id.to_string()];
    assert_eq!(github.declared.cause, RefusalCause::Disabled);
    assert_eq!(github.webhook_name, "github");

    crate::test_support::teardown_test_db(&db).await;
}

#[tokio::test]
async fn the_age_of_a_retraction_spans_the_run_and_not_the_declaration() {
    let (pool, db) = crate::test_support::setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let created = seed_hook(&pool, &bus, "github").await;

    let four_days = 4 * 86_400;
    bus.emit(BusEvent::System(refused_event(
        created.id,
        "github",
        RefusalCause::Disabled,
        four_days,
    )))
    .await
    .unwrap();

    let standing = declared_refusals(&pool).await.unwrap();
    let github = &standing[&created.id.to_string()];
    assert!(
        github.refusing_secs >= four_days,
        "expected at least the run's own {four_days}s, got {}",
        github.refusing_secs
    );

    crate::test_support::teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_payload_this_engine_cannot_read_declares_nothing() {
    let (pool, db) = crate::test_support::setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let created = seed_hook(&pool, &bus, "github").await;

    async fn stored(
        pool: &sqlx::PgPool,
        webhook_id: Uuid,
        data: serde_json::Value,
    ) -> HashMap<String, StandingRefusal> {
        sqlx::query("DELETE FROM events WHERE event_type = 'WebhookDeliveriesRefused'")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO events (id, event_type, payload, aggregate, aggregate_id) \
             VALUES ($1, 'WebhookDeliveriesRefused', $2, 'webhook', $3)",
        )
        .bind(Uuid::new_v4())
        .bind(serde_json::json!({ "type": "WebhookDeliveriesRefused", "data": data }))
        .bind(webhook_id.to_string())
        .execute(pool)
        .await
        .unwrap();
        declared_refusals(pool).await.unwrap()
    }

    assert!(stored(
        &pool,
        created.id,
        serde_json::json!({
            "webhook_id": created.id.to_string(),
            "webhook_name": "github",
            "cause": "from-the-future",
            "refusing_since": ago(RUN_SECS).to_rfc3339(),
        })
    )
    .await
    .is_empty());

    assert!(stored(
        &pool,
        created.id,
        serde_json::json!({
            "webhook_id": created.id.to_string(),
            "webhook_name": "github",
            "cause": "disabled",
            "refusing_since": "whenever",
        })
    )
    .await
    .is_empty());

    crate::test_support::teardown_test_db(&db).await;
}
