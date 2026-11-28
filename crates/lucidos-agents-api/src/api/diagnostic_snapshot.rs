//! INVARIANT: Bounded private evidence for the gateway bundle builder. Never a send path.
use super::{ApiError, AppState};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::{DateTime, Utc};
use lucidos_diagnostic_log::{private, snapshot::Snapshot, writer::Record};
use sqlx::Row;

const FAILURE_TYPES: &[&str] = &[
    "ResponseFailed",
    "EngineSupervisorRespawned",
    "InternalFaultRecorded",
];
use lucidos_diagnostic_log::snapshot::{
    FAILURE_WINDOW_DAYS, MAX_CURRENT_FAULTS, MAX_EVENT_PAYLOAD_BYTES, MAX_FAILURE_EVENTS,
};

fn event_record(id: uuid::Uuid, ts: DateTime<Utc>, kind: &str, payload: String) -> Record {
    let kind = FAILURE_TYPES
        .iter()
        .copied()
        .find(|known| *known == kind)
        .unwrap_or("unknown");
    let line = lucidos_diagnostic_log::Line::typed(
        "event",
        vec![
            lucidos_diagnostic_log::Fragment::literal("event "),
            lucidos_diagnostic_log::Fragment::literal(kind),
            lucidos_diagnostic_log::Fragment::value(&id, format!(" {id} ")),
            lucidos_diagnostic_log::Fragment::value(&private(&payload), payload.clone()),
        ],
    );
    Record {
        ts: ts.to_rfc3339(),
        pid: 0,
        label: "Event".into(),
        msg: String::new(),
        line: Some(line),
    }
}

pub(super) async fn snapshot(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Snapshot>, ApiError> {
    if !super::local_auth::is_local_process(&headers) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Diagnostic snapshots require a local process",
        ));
    }
    crate::internal_faults::synchronize().await.map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Fault snapshot unavailable",
        )
    })?;
    collect(&state).await.map(Json).map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Diagnostic snapshot unavailable",
        )
    })
}

async fn collect(state: &AppState) -> Result<Snapshot, Box<dyn std::error::Error + Send + Sync>> {
    // INVARIANT: Bound each payload in SQL before materializing it; even the owner's copy
    // must not read an unbounded conversation/tool payload into this response.
    let rows = evidence_rows(&state.pool).await?;
    let mut snapshot = Snapshot {
        healthy: state.engine.database_health().is_reachable() && !state.engine.is_shutting_down(),
        ..Default::default()
    };
    for row in rows {
        let kind: String = row.try_get("event_type")?;
        let record = event_record(
            row.try_get("id")?,
            row.try_get("created")?,
            &kind,
            row.try_get("payload")?,
        );
        if kind == "InternalFaultRecorded" {
            let payload: String = row.try_get("payload")?;
            if let Ok(crate::engine::event_bus::SystemEvent::InternalFaultRecorded { kind }) =
                serde_json::from_str::<crate::engine::event_bus::SystemEvent>(&payload)
            {
                let mut record = record;
                record.line = Some(lucidos_diagnostic_log::Line::typed(
                    "internal fault",
                    vec![
                        lucidos_diagnostic_log::Fragment::literal("internal fault "),
                        lucidos_diagnostic_log::Fragment::literal(kind.as_str()),
                    ],
                ));
                snapshot.faults.push(record);
                continue;
            }
            snapshot.faults.push(record);
        } else {
            snapshot.events.push(record);
        }
    }
    snapshot.versions.insert(
        "engine".into(),
        crate::build_identity().engine_version.into(),
    );
    snapshot
        .versions
        .insert("engine-build".into(), crate::build_identity().id.into());
    snapshot
        .versions
        .insert("release".into(), crate::build_identity().release.into());
    snapshot
        .configured
        .insert("llm".into(), state.engine.llm_configured());
    for provider in state.engine.configured_providers().unwrap_or_default() {
        snapshot
            .configured
            .insert(format!("provider-{provider}"), true);
    }
    Ok(snapshot)
}

async fn evidence_rows(pool: &sqlx::PgPool) -> Result<Vec<sqlx::postgres::PgRow>, sqlx::Error> {
    sqlx::query(
        "(SELECT id, created, event_type, CASE WHEN octet_length(payload::text) <= $4 THEN payload::text ELSE '[payload omitted: exceeds snapshot bound]' END AS payload, sequence FROM events
          WHERE event_type = ANY($1) AND event_type <> 'InternalFaultRecorded'
          AND created >= now() - make_interval(days => $3) ORDER BY sequence DESC LIMIT $2)
         UNION ALL
         (SELECT id, created, event_type, CASE WHEN octet_length(payload::text) <= $4 THEN payload::text ELSE '[payload omitted: exceeds snapshot bound]' END AS payload, sequence FROM events
          WHERE event_type = 'InternalFaultRecorded' AND sequence >
          (SELECT COALESCE(MAX(sequence), 0) FROM events WHERE event_type = 'DiagnosticBundleSent')
          ORDER BY sequence DESC LIMIT $5) ORDER BY sequence"
    ).bind(FAILURE_TYPES).bind(MAX_FAILURE_EVENTS).bind(FAILURE_WINDOW_DAYS).bind(MAX_EVENT_PAYLOAD_BYTES).bind(MAX_CURRENT_FAULTS).fetch_all(pool).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use lucidos_diagnostic_log::Destination;
    #[test]
    fn event_payloads_and_unknown_names_are_private() {
        let r = event_record(
            uuid::Uuid::nil(),
            Utc::now(),
            "PRIVATE_KIND_MARKER",
            "PRIVATE_PAYLOAD_MARKER token=opaque-value".into(),
        );
        let maintainer = r.render(Destination::Maintainer);
        assert!(maintainer.contains("unknown"));
        assert!(!maintainer.contains("PRIVATE_"));
        assert!(r
            .render(Destination::Owner)
            .contains("PRIVATE_PAYLOAD_MARKER"));
        assert!(!serde_json::to_string(&r).unwrap().contains("opaque-value"));
    }
    #[tokio::test]
    async fn diagnostic_snapshot_reads_only_failures_and_current_faults() {
        use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
        use crate::internal_faults::FaultKind;
        let (pool, db) = crate::test_support::setup_test_db().await;
        let (bus, _) = EventBus::new(pool.clone());
        bus.emit(BusEvent::System(SystemEvent::InternalFaultRecorded {
            kind: FaultKind::Panic,
        }))
        .await
        .unwrap();
        let rows = evidence_rows(&pool).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].get::<String, _>("event_type"),
            "InternalFaultRecorded"
        );
        bus.emit(BusEvent::System(SystemEvent::DiagnosticBundleSent {
            destination: "maintainer".into(),
            report_code: None,
            size_bytes: 5,
            sources: vec![],
            actor: None,
        }))
        .await
        .unwrap();
        assert!(evidence_rows(&pool).await.unwrap().is_empty());
        bus.emit(BusEvent::System(SystemEvent::InternalFaultRecorded {
            kind: FaultKind::TaskPanic,
        }))
        .await
        .unwrap();
        assert_eq!(evidence_rows(&pool).await.unwrap().len(), 1);
        for i in 0..=MAX_FAILURE_EVENTS {
            bus.emit(BusEvent::System(SystemEvent::DomainEvent {
                event_type: "ResponseFailed".into(),
                payload: serde_json::json!({"index": i, "message": "PRIVATE_EVENT_MARKER".repeat(150)}),
                depth: 0, transient: false, actor: None,
            })).await.unwrap();
        }
        let rows = evidence_rows(&pool).await.unwrap();
        assert_eq!(rows.len(), MAX_FAILURE_EVENTS as usize + 1);
        let failure_rows = rows
            .iter()
            .filter(|r| r.get::<String, _>("event_type") == "ResponseFailed")
            .collect::<Vec<_>>();
        assert_eq!(failure_rows.len(), MAX_FAILURE_EVENTS as usize);
        for row in failure_rows {
            assert_eq!(
                row.get::<String, _>("payload"),
                "[payload omitted: exceeds snapshot bound]"
            );
        }
        crate::test_support::teardown_test_db(&db).await;
    }
}
