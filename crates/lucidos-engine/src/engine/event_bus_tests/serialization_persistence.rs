use super::super::*;
use super::*;

#[test]
fn notification_created_is_persisted() {
    let event = SystemEvent::NotificationCreated {
        id: "n-1".into(),
        title: "Test".into(),
        message: "Hello".into(),
        task_id: Some("t-1".into()),
        app_id: Some("morning-brief".into()),
        thread_id: Some("11111111-1111-1111-1111-111111111111".into()),
        event_id: None,
        tap: crate::scheduler::notifications::Tap::Modal,
        actor: None,
    };
    assert!(event.is_persisted());
    assert_eq!(event.aggregate(), "notification");
    assert_eq!(event.aggregate_id(), "n-1");
}

#[test]
fn notification_created_skips_none_fields_in_sse() {
    let emitted = EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: None,
        created: Utc::now(),
        typed: BusEvent::System(SystemEvent::NotificationCreated {
            id: "n-2".into(),
            title: "Alert".into(),
            message: "Something happened".into(),
            task_id: None,
            app_id: None,
            thread_id: None,
            event_id: None,
            tap: crate::scheduler::notifications::Tap::Modal,
            actor: None,
        }),
        aggregate: None,
    };
    let json: serde_json::Value = serde_json::from_str(&emitted.to_sse_json()).unwrap();
    assert!(
        json["data"].get("task_id").is_none(),
        "None task_id should be skipped"
    );
    assert!(
        json["data"].get("app_id").is_none(),
        "None app_id should be skipped"
    );
    assert!(
        json["data"].get("thread_id").is_none(),
        "None thread_id should be skipped"
    );
}

#[test]
fn notification_created_carries_thread_id_in_sse() {
    let emitted = EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: None,
        created: Utc::now(),
        typed: BusEvent::System(SystemEvent::NotificationCreated {
            id: "n-3".into(),
            title: "Workspace learning".into(),
            message: "Report at artifacts/...".into(),
            task_id: None,
            app_id: None,
            thread_id: Some("22222222-2222-2222-2222-222222222222".into()),
            event_id: None,
            tap: crate::scheduler::notifications::Tap::Modal,
            actor: None,
        }),
        aggregate: None,
    };
    let json: serde_json::Value = serde_json::from_str(&emitted.to_sse_json()).unwrap();
    assert_eq!(
        json["data"]["thread_id"], "22222222-2222-2222-2222-222222222222",
        "thread_id should ride along on the SSE frame so the inbox can deep-link"
    );
}

#[test]
fn preferences_changed_is_persisted_with_key_value() {
    let set = SystemEvent::PreferencesChanged {
        key: "language".into(),
        value: Some("nb".into()),
        actor: None,
    };
    assert!(set.is_persisted());
    assert_eq!(set.aggregate(), "preference");
    assert_eq!(set.event_type(), "PreferencesChanged");

    // to_payload() wraps in serde's tag/content envelope: { "type": "...", "data": { ... } }
    let json = set.to_payload();
    assert_eq!(json["data"]["key"], "language");
    assert_eq!(json["data"]["value"], "nb");
}

#[test]
fn preferences_changed_delete_has_null_value() {
    let del = SystemEvent::PreferencesChanged {
        key: "old_setting".into(),
        value: None,
        actor: None,
    };
    assert!(del.is_persisted());

    let json = del.to_payload();
    assert_eq!(json["data"]["key"], "old_setting");
    // value is None → skipped by skip_serializing_if
    assert!(
        json["data"].get("value").is_none(),
        "None value should be skipped"
    );
}

#[test]
fn other_system_events_not_persisted() {
    assert!(!SystemEvent::NotificationRead {
        id: "n-1".into(),
        actor: None
    }
    .is_persisted());
    assert!(!SystemEvent::NotificationsAllRead { actor: None }.is_persisted());
    assert!(!SystemEvent::EmbeddingModelStatusChanged {
        model_id: "multilingual-e5-small".into(),
        load_state: crate::memory::EmbeddingModelLoadState::Downloading {
            downloaded_bytes: 1,
            total_bytes: 2,
        },
    }
    .is_persisted());
    assert!(!SystemEvent::MemoryRebuildProgress {
        processed: 0,
        total: 0,
        percent: 0
    }
    .is_persisted());
    assert!(!SystemEvent::Toast {
        message: "hi".into(),
        level: "info".into()
    }
    .is_persisted());
}

#[test]
fn trigger_created_serializes_for_sse() {
    let event = SystemEvent::TriggerCreated {
        trigger_id: "abc".to_string(),
        payload: serde_json::json!({"name": "Test"}),
        actor: None,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "TriggerCreated");
    assert_eq!(json["data"]["trigger_id"], "abc");
    assert!(event.is_persisted());
}

#[test]
fn app_created_serializes_for_sse() {
    let event = SystemEvent::AppCreated {
        app_id: "my-app".to_string(),
        name: Some("My App".to_string()),
        actor: None,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "AppCreated");
    assert_eq!(json["data"]["app_id"], "my-app");
    assert!(!event.is_persisted());
}

#[test]
fn app_deleted_serializes_for_sse() {
    let event = SystemEvent::AppDeleted {
        app_id: "habit-tracker".to_string(),
        actor: None,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "AppDeleted");
    assert!(!event.is_persisted());
}

#[test]
fn artifact_created_serializes_and_persists() {
    let event = SystemEvent::ArtifactCreated {
        artifact_path: "notes/todo.md".to_string(),
        commit: "abc1234".to_string(),
        source: Some("run_python".to_string()),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "ArtifactCreated");
    assert_eq!(json["data"]["artifact_path"], "notes/todo.md");
    assert_eq!(json["data"]["commit"], "abc1234");
    assert_eq!(json["data"]["source"], "run_python");
    assert!(event.is_persisted());
    assert_eq!(event.aggregate(), "artifact");
    assert_eq!(event.aggregate_id(), "notes/todo.md");
}

#[test]
fn artifact_updated_skips_none_source() {
    let event = SystemEvent::ArtifactUpdated {
        artifact_path: "report.md".to_string(),
        commit: "def5678".to_string(),
        source: None,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "ArtifactUpdated");
    assert!(
        json["data"].get("source").is_none(),
        "None source should be skipped"
    );
    assert!(event.is_persisted());
}

#[test]
fn artifact_deleted_serializes_and_persists() {
    let event = SystemEvent::ArtifactDeleted {
        artifact_path: "old.txt".to_string(),
        commit: "aaa1111".to_string(),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "ArtifactDeleted");
    assert!(event.is_persisted());
    assert_eq!(event.aggregate(), "artifact");
}

#[test]
fn language_set_serializes_and_persists() {
    let event = SystemEvent::LanguageSet {
        language: "nb".to_string(),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "LanguageSet");
    assert_eq!(json["data"]["language"], "nb");
    assert!(event.is_persisted());
    assert_eq!(event.aggregate(), "preference");
}

#[test]
fn timezone_set_serializes_and_persists() {
    let event = SystemEvent::TimezoneSet {
        timezone: "Europe/Oslo".to_string(),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "TimezoneSet");
    assert_eq!(json["data"]["timezone"], "Europe/Oslo");
    assert!(event.is_persisted());
    assert_eq!(event.aggregate(), "preference");
}

#[test]
fn repository_imported_serializes_and_persists() {
    let event = SystemEvent::RepositoryImported {
        url: "https://github.com/user/repo".to_string(),
        branch: "main".to_string(),
        destination: "repo".to_string(),
        file_count: 42,
        skipped_count: 3,
        commit: "bbb2222".to_string(),
        files: vec!["README.md".to_string(), "src/main.rs".to_string()],
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "RepositoryImported");
    assert_eq!(json["data"]["file_count"], 42);
    assert_eq!(json["data"]["files"].as_array().unwrap().len(), 2);
    assert!(event.is_persisted());
    assert_eq!(event.aggregate(), "artifact");
    assert_eq!(event.aggregate_id(), "repo");
}

#[test]
fn trigger_completed_serializes_and_persists() {
    let event = SystemEvent::TriggerCompleted {
        trigger_id: "task-123".to_string(),
        trigger_name: "Daily backup".to_string(),
        result_summary: "Completed successfully".to_string(),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "TriggerCompleted");
    assert_eq!(json["data"]["trigger_id"], "task-123");
    assert!(event.is_persisted());
    assert_eq!(event.aggregate(), "trigger");
    assert_eq!(event.aggregate_id(), "task-123");
}

#[test]
fn change_discarded_serializes_and_persists() {
    let event = SystemEvent::ChangeDiscarded {
        change_id: "c-456".to_string(),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "ChangeDiscarded");
    assert_eq!(json["data"]["change_id"], "c-456");
    assert!(event.is_persisted());
    assert_eq!(event.aggregate(), "change");
    assert_eq!(event.aggregate_id(), "c-456");
}

#[test]
fn email_sent_persists_envelope_metadata_without_body() {
    let event = SystemEvent::EmailSent {
        account: "primary".to_string(),
        to: vec![
            "alice@example.com".to_string(),
            "bob@example.com".to_string(),
        ],
        cc: vec!["carol@example.com".to_string()],
        bcc: vec![],
        subject: "Q3 numbers".to_string(),
        attachment_count: 2,
        actor: None,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "EmailSent");
    assert_eq!(json["data"]["account"], "primary");
    assert_eq!(json["data"]["to"][0], "alice@example.com");
    assert_eq!(json["data"]["to"][1], "bob@example.com");
    assert_eq!(json["data"]["cc"][0], "carol@example.com");
    assert_eq!(json["data"]["subject"], "Q3 numbers");
    assert_eq!(json["data"]["attachment_count"], 2);
    // Body is intentionally NOT broadcast — it's user data.
    assert!(
        json["data"].get("body").is_none(),
        "EmailSent must never carry the message body"
    );
    // Empty bcc is skipped to keep the wire shape tight.
    assert!(
        json["data"].get("bcc").is_none(),
        "Empty bcc Vec should be omitted from the payload"
    );
    assert!(event.is_persisted());
    assert_eq!(event.aggregate(), "email");
    assert_eq!(event.aggregate_id(), "primary");
}

#[test]
fn email_sent_omits_empty_cc_and_bcc() {
    let event = SystemEvent::EmailSent {
        account: "secondary".to_string(),
        to: vec!["bob@example.com".to_string()],
        cc: vec![],
        bcc: vec![],
        subject: String::new(),
        attachment_count: 0,
        actor: None,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert!(json["data"].get("cc").is_none());
    assert!(json["data"].get("bcc").is_none());
}

#[test]
fn proxy_modules_reloaded_persists_with_names_list() {
    let event = SystemEvent::ProxyModulesReloaded {
        count: 2,
        names: vec!["binance-hmac".to_string(), "test-echo".to_string()],
        actor: None,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "ProxyModulesReloaded");
    assert_eq!(json["data"]["count"], 2);
    assert_eq!(json["data"]["names"][0], "binance-hmac");
    assert_eq!(json["data"]["names"][1], "test-echo");
    assert!(event.is_persisted());
    assert_eq!(event.aggregate(), "proxy_modules");
    // No per-row identity — there's one auth-modules map globally.
    assert_eq!(event.aggregate_id(), "global");
}

#[test]
fn proxy_modules_reloaded_handles_empty_reload() {
    // The auth-modules directory may legitimately be empty; the audit
    // row should still write so the timeline records the swap.
    let event = SystemEvent::ProxyModulesReloaded {
        count: 0,
        names: vec![],
        actor: None,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["data"]["count"], 0);
    assert_eq!(json["data"]["names"].as_array().unwrap().len(), 0);
    assert!(event.is_persisted());
}

#[test]
fn domain_event_is_persisted_with_inner_payload() {
    let event = SystemEvent::DomainEvent {
        event_type: "SlideTextEdited".to_string(),
        payload: serde_json::json!({"slide_id": 1, "summary": "Updated title"}),
        depth: 0,
        transient: false,
        actor: None,
    };
    assert!(event.is_persisted());
    assert_eq!(event.aggregate(), "domain");
    assert_eq!(event.aggregate_id(), "SlideTextEdited");
    // Must not double-wrap — DB and SSE consumers expect the raw inner payload
    let payload = event.to_payload();
    assert_eq!(payload["slide_id"], 1);
    assert_eq!(payload["summary"], "Updated title");
}

/// Transient domain events (e.g. SlidePresenterState heartbeats every 3s)
/// must be broadcast on SSE but skip the events table — otherwise we'd
/// write ~1,200 rows/hour for a single Super Slides session.
#[test]
fn transient_domain_event_is_not_persisted() {
    let event = SystemEvent::DomainEvent {
        event_type: "SlidePresenterState".to_string(),
        payload: serde_json::json!({"slide_index": 5}),
        depth: 0,
        transient: true,
        actor: None,
    };
    assert!(
        !event.is_persisted(),
        "transient domain events must not be written to the events table"
    );
}

/// Transient flag is internal control state — must not appear on the SSE
/// wire format (frontend listeners shouldn't have to know about it).
#[test]
fn transient_domain_event_sse_json_uses_inner_event_type() {
    let emitted = EmittedEvent {
        event_id: Uuid::new_v4(),
        seq: None,
        created: Utc::now(),
        typed: BusEvent::System(SystemEvent::DomainEvent {
            event_type: "SlidePresenterPing".into(),
            payload: serde_json::json!({"timestamp": 1_700_000_000}),
            depth: 0,
            transient: true,
            actor: None,
        }),
        aggregate: None,
    };

    let json: serde_json::Value = serde_json::from_str(&emitted.to_sse_json()).unwrap();
    assert_eq!(
        json["type"], "SlidePresenterPing",
        "Wire `type` must be the inner event_type, even for transient events"
    );
    assert_eq!(json["data"]["timestamp"], 1_700_000_000);
    assert!(
        json["data"].get("transient").is_none(),
        "The transient flag is internal — it must not leak to the SSE wire format"
    );
}

/// Phase 4: per-turn idle is signaled by `CodingAgentIdled`, NOT
/// `SessionEnded`. ChangeProposed must appear before the idle marker so the
/// frontend can render "Done · 1 change" the moment idle fires. SessionEnded
/// is now reserved for terminal events (Shutdown / Panic / Closed) and never
/// appears in the normal turn sequence.
#[test]
fn coding_agent_idled_is_last_event_after_change_proposed() {
    let (tx, mut rx) = broadcast::channel::<EmittedEvent>(16);
    let tid = Uuid::new_v4();

    // Correct event ordering for a CC turn that proposes a change:
    // ResponseGenerated → ChangeProposed → CodingAgentIdled
    let events = vec![
        ThreadEvent::ResponseGenerated {
            text: "Done.".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        ThreadEvent::ChangeProposed {
            change_id: "c-1".into(),
            description: Some("Fix bug".into()),
            files: vec!["src/main.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: String::new(),
            repo_root: String::new(),
            hardened: false,
            incomplete: false,
            path: String::new(),
            diff: String::new(),
        },
        ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
        },
    ];

    for event in &events {
        let _ = tx.send(EmittedEvent {
            event_id: Uuid::new_v4(),
            seq: Some(0), // seq doesn't matter for ordering test
            created: Utc::now(),
            typed: BusEvent::Thread {
                thread_id: tid,
                event: event.clone(),
                meta: EventMeta::NONE,
            },
            aggregate: None,
        });
    }

    let mut received = vec![];
    while let Ok(e) = rx.try_recv() {
        if let BusEvent::Thread { event, .. } = &e.typed {
            received.push(event.event_type().to_string());
        }
    }

    assert_eq!(
        received,
        vec!["ResponseGenerated", "ChangeProposed", "CodingAgentIdled"]
    );

    assert_eq!(
        received.last().unwrap(),
        "CodingAgentIdled",
        "CodingAgentIdled must be the final event — ChangeProposed must come before it"
    );
}

/// The persistence half of the HTML-entity bug hunt
/// (`docs/plans/2026-08-09-tool-arg-html-entity-repair.md`): a tool argument
/// holding `& < > " '` must reach the `events` row byte-identical, and come
/// back out the same way.
///
/// The reported corruption was visible in persisted `ToolCalled` args, so
/// Postgres and the jsonb round-trip were suspects alongside the provider
/// stream. Neither escapes: this pins that, so a future regression in the
/// write path cannot hide behind `engine::tool_arg_entity_repair` fixing the
/// label arguments while silently mangling the rest.
#[tokio::test]
async fn tool_called_args_persist_special_characters_verbatim() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    // A plain-text label alongside markup the model wrote on purpose: after
    // the repair the first is literal and the second keeps its escaping, and
    // persistence must not touch either.
    let name = "Machine & Tooling <Health> \"q\" 'a'";
    let html = "<p>Tools &amp; Toys</p>";

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ToolCalled {
            name: "trigger_groups".into(),
            args: serde_json::json!({"name": name, "html_content": html}),
            description: format!("Creating trigger group '{name}'..."),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let payload: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM events WHERE thread_id = $1 AND event_type = 'ToolCalled'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(
        payload["args"]["name"], name,
        "the persisted argument must hold the literal characters, not entities"
    );
    assert_eq!(
        payload["args"]["html_content"], html,
        "markup the model escaped on purpose must survive escaped"
    );
    assert_eq!(
        payload["description"],
        format!("Creating trigger group '{name}'..."),
        "the derived description is persisted verbatim too"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
