use super::*;

fn headers_with(pairs: &[(&str, &str)]) -> axum::http::HeaderMap {
    let mut h = axum::http::HeaderMap::new();
    for (k, v) in pairs {
        h.insert(
            axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
            axum::http::HeaderValue::from_str(v).unwrap(),
        );
    }
    h
}

fn caller(
    name: &str,
    thread_id: Option<Uuid>,
    event_id: Option<Uuid>,
    mode: ActorMode,
) -> Option<CallerOrigin> {
    Some(CallerOrigin {
        workspace: name.into(),
        thread_id,
        event_id,
        mode,
    })
}

#[test]
fn build_origin_user_with_caller_yields_workspace() {
    let src_thread = Uuid::new_v4();
    let src_event = Uuid::new_v4();
    let h = headers_with(&[("user-agent", "lucidos-engine/0.1")]);
    let origin = build_message_origin(
        &h,
        ActorMode::Human,
        Some("dev-1"),
        Some("dev label".into()),
        None,
        None,
        None,
        caller("dev", Some(src_thread), Some(src_event), ActorMode::Human),
    );
    match origin {
        Some(MessageOrigin::Workspace {
            workspace,
            thread_id,
            event_id,
            user_agent,
            mode: _,
        }) => {
            assert_eq!(workspace, "dev");
            assert_eq!(thread_id, Some(src_thread));
            assert_eq!(event_id, Some(src_event));
            assert_eq!(user_agent.as_deref(), Some("lucidos-engine/0.1"));
        }
        other => panic!("expected Workspace, got {:?}", other),
    }
}

#[test]
fn build_origin_user_caller_takes_precedence_over_device() {
    let h = headers_with(&[]);
    let origin = build_message_origin(
        &h,
        ActorMode::Human,
        Some("dev-1"),
        Some("Chrome".into()),
        None,
        None,
        None,
        caller("myws", None, None, ActorMode::Human),
    );
    assert!(matches!(origin, Some(MessageOrigin::Workspace { .. })));
}

#[test]
fn build_origin_user_with_device_id_yields_device_with_label() {
    let h = headers_with(&[]);
    let origin = build_message_origin(
        &h,
        ActorMode::Human,
        Some("dev-1"),
        Some("Chrome on Mac".into()),
        None,
        None,
        None,
        None,
    );
    match origin {
        Some(MessageOrigin::Device { device_id, label }) => {
            assert_eq!(device_id, "dev-1");
            assert_eq!(label, "Chrome on Mac");
        }
        other => panic!("expected Device, got {:?}", other),
    }
}

#[test]
fn build_origin_user_with_device_id_no_label_falls_back_to_short_id() {
    let h = headers_with(&[]);
    let origin = build_message_origin(
        &h,
        ActorMode::Human,
        Some("abcdef0123456789"),
        None,
        None,
        None,
        None,
        None,
    );
    match origin {
        Some(MessageOrigin::Device { label, .. }) => assert_eq!(label, "device-abcdef01"),
        other => panic!("expected Device, got {:?}", other),
    }
}

#[test]
fn build_origin_user_short_label_handles_short_id() {
    // Defensive: device_id shorter than 8 chars must not panic.
    let h = headers_with(&[]);
    let origin = build_message_origin(
        &h,
        ActorMode::Human,
        Some("ab"),
        None,
        None,
        None,
        None,
        None,
    );
    match origin {
        Some(MessageOrigin::Device { label, .. }) => assert_eq!(label, "device-ab"),
        other => panic!("expected Device, got {:?}", other),
    }
}

#[test]
fn build_origin_user_no_device_no_caller_yields_api() {
    let h = headers_with(&[("user-agent", "curl/8")]);
    let origin = build_message_origin(&h, ActorMode::Human, None, None, None, None, None, None);
    match origin {
        Some(MessageOrigin::Api {
            user_agent,
            mode,
            source_thread_id,
        }) => {
            assert_eq!(user_agent.as_deref(), Some("curl/8"));
            assert_eq!(mode, ActorMode::Human);
            assert_eq!(source_thread_id, None);
        }
        other => panic!("expected Api, got {:?}", other),
    }
}

#[test]
fn build_origin_human_mode_with_caller_yields_workspace_human_mode() {
    let h = headers_with(&[]);
    let origin = build_message_origin(
        &h,
        ActorMode::Human,
        None,
        None,
        None,
        None,
        None,
        caller("dev", None, None, ActorMode::Human),
    );
    match origin {
        Some(MessageOrigin::Workspace {
            workspace, mode, ..
        }) => {
            assert_eq!(workspace, "dev");
            assert_eq!(mode, ActorMode::Human);
        }
        other => panic!("expected Workspace, got {:?}", other),
    }
}

#[test]
fn build_origin_agent_mode_with_caller_yields_workspace_agent_mode() {
    // CallerOrigin carries the upstream mode — the calling workspace's
    // actor (Agent here) flows straight into the Workspace branch.
    let h = headers_with(&[]);
    let origin = build_message_origin(
        &h,
        ActorMode::Agent,
        None,
        None,
        None,
        None,
        None,
        caller("dev", None, None, ActorMode::Agent),
    );
    match origin {
        Some(MessageOrigin::Workspace { mode, .. }) => assert_eq!(mode, ActorMode::Agent),
        other => panic!("expected Workspace agent mode, got {:?}", other),
    }
}

#[test]
fn build_origin_engine_mode_with_caller_yields_workspace_engine_mode() {
    let h = headers_with(&[]);
    let origin = build_message_origin(
        &h,
        ActorMode::Engine,
        None,
        None,
        None,
        None,
        None,
        caller("dev", None, None, ActorMode::Engine),
    );
    match origin {
        Some(MessageOrigin::Workspace { mode, .. }) => assert_eq!(mode, ActorMode::Engine),
        other => panic!("expected Workspace engine mode, got {:?}", other),
    }
}

#[test]
fn build_origin_agent_mode_with_parent_thread_yields_thread_link_parent_agent_mode() {
    let parent_id = Uuid::new_v4();
    let origin = build_message_origin(
        &headers_with(&[]),
        ActorMode::Agent,
        None,
        None,
        Some(parent_id),
        Some("parent".into()),
        None,
        None,
    );
    match origin {
        Some(MessageOrigin::ThreadLink {
            thread_id,
            mode,
            direction,
            ..
        }) => {
            assert_eq!(thread_id, parent_id);
            assert_eq!(mode, ActorMode::Agent);
            assert_eq!(direction, ThreadDirection::Parent);
        }
        other => panic!("expected ThreadLink, got {:?}", other),
    }
}

#[test]
fn build_origin_engine_mode_with_no_parent_yields_none() {
    // Engine-initiated work without a parent thread context returns None;
    // callers must construct MessageOrigin::Engine { reason } directly.
    let origin = build_message_origin(
        &headers_with(&[]),
        ActorMode::Engine,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    assert!(origin.is_none());
}

#[test]
fn build_origin_engine_mode_with_parent_thread_yields_thread_link_engine_mode() {
    let parent_id = Uuid::new_v4();
    let origin = build_message_origin(
        &headers_with(&[]),
        ActorMode::Engine,
        None,
        None,
        Some(parent_id),
        None,
        None,
        None,
    );
    match origin {
        Some(MessageOrigin::ThreadLink {
            mode, direction, ..
        }) => {
            assert_eq!(mode, ActorMode::Engine);
            assert_eq!(direction, ThreadDirection::Parent);
        }
        other => panic!("expected ThreadLink, got {:?}", other),
    }
}

#[test]
fn user_actor_convenience_passes_through_to_build_message_origin() {
    let h = headers_with(&[("user-agent", "curl/8")]);
    let actor = user_actor(&h, Some("dev-1"), Some("Chrome".into()));
    match actor {
        Some(MessageOrigin::Device { device_id, label }) => {
            assert_eq!(device_id, "dev-1");
            assert_eq!(label, "Chrome");
        }
        other => panic!("expected Device, got {:?}", other),
    }
}

#[test]
fn user_actor_falls_back_to_header_device_id_when_no_explicit_id() {
    let h = headers_with(&[("x-lucidos-device-id", "abcdef0123456789")]);
    let actor = user_actor(&h, None, None);
    match actor {
        Some(MessageOrigin::Device { device_id, label }) => {
            assert_eq!(device_id, "abcdef0123456789");
            assert_eq!(label, "device-abcdef01");
        }
        other => panic!("expected Device from header, got {:?}", other),
    }
}

#[test]
fn user_actor_explicit_id_takes_precedence_over_header() {
    let h = headers_with(&[("x-lucidos-device-id", "dev-from-header")]);
    let actor = user_actor(&h, Some("dev-from-arg"), Some("Chrome".into()));
    match actor {
        Some(MessageOrigin::Device { device_id, label }) => {
            assert_eq!(device_id, "dev-from-arg");
            assert_eq!(label, "Chrome");
        }
        other => panic!("expected explicit Device, got {:?}", other),
    }
}

#[tokio::test]
async fn user_actor_resolved_enriches_device_label_from_db() {
    // Regression: every mutating handler used to call `user_actor(.., None, None)`
    // and never looked up the label, so events stamped Device { label: <fallback> }
    // — visible as the bare `device-<short>` placeholder in the actor popover.
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    crate::test_support::seed_device(
        &pool,
        "test-device-1",
        Some("Mozilla/5.0"),
        Some("My MacBook"),
    )
    .await;

    let h = headers_with(&[("x-lucidos-device-id", "test-device-1")]);
    let actor = user_actor_resolved(&h, &pool, None).await;
    match actor {
        Some(MessageOrigin::Device { device_id, label }) => {
            assert_eq!(device_id, "test-device-1");
            assert_eq!(
                label, "My MacBook",
                "label must come from devices table, not the device-<short> fallback"
            );
        }
        other => panic!("expected Device with db-looked-up label, got {:?}", other),
    }

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn user_actor_resolved_uses_explicit_device_id_override() {
    // The settings endpoint takes device_id from the request body, not the header.
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    crate::test_support::seed_device(&pool, "from-body", None, Some("Body Device")).await;

    let h = headers_with(&[("x-lucidos-device-id", "from-header")]);
    let actor = user_actor_resolved(&h, &pool, Some("from-body")).await;
    match actor {
        Some(MessageOrigin::Device { device_id, label }) => {
            assert_eq!(device_id, "from-body", "explicit override beats header");
            assert_eq!(label, "Body Device");
        }
        other => panic!("expected Device using override, got {:?}", other),
    }

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn user_actor_resolved_no_caller_no_device_yields_api() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let h = headers_with(&[("user-agent", "curl/8")]);
    let actor = user_actor_resolved(&h, &pool, None).await;
    match actor {
        Some(MessageOrigin::Api {
            user_agent,
            mode,
            source_thread_id,
        }) => {
            assert_eq!(user_agent.as_deref(), Some("curl/8"));
            assert_eq!(mode, ActorMode::Human);
            assert_eq!(source_thread_id, None);
        }
        other => panic!("expected Api, got {:?}", other),
    }
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn user_actor_resolved_unknown_device_id_falls_back_to_short_label() {
    // Device id in header but row missing in DB — falls back via
    // build_message_origin's `device-<short>` derivation, never panics.
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let h = headers_with(&[("x-lucidos-device-id", "no-such-device")]);
    let actor = user_actor_resolved(&h, &pool, None).await;
    match actor {
        Some(MessageOrigin::Device { device_id, label }) => {
            assert_eq!(device_id, "no-such-device");
            assert_eq!(label, "device-no-such-");
        }
        other => panic!("expected Device with fallback label, got {:?}", other),
    }
    crate::test_support::teardown_test_db(&db_name).await;
}

// ── subprocess_origin / agent origin token ──────────────────────────────
//
// These tests cover the engine's defense against the incident where an
// LLM agent ran `curl -X POST /api/v1/changes/<id>/apply` from inside its
// `run_bash` shell and the resulting `ChangeApplied` event was stamped
// `actor: { kind: api, mode: human, user_agent: "curl/8.7.1" }` — which
// the chat-thread UI rendered as a "You" card in the receiving thread.
// The fix: a Lucidos-spawned subprocess gets `LUCIDOS_AGENT_ORIGIN_TOKEN`
// in its env. The `lucidos` CLI (and the engine when it shells out for
// subprocess tools) forwards that value as the
// `x-lucidos-agent-origin-token` header. The engine recognises the token
// and stamps `MessageOrigin::Api { mode: Agent, source_thread_id }` so
// the timeline says "Lucidos Agent" (not "You") and the popover links
// back to the spawning thread.

/// Helper: install a known token for the duration of the test.
/// `init_agent_origin_token` is idempotent (`OnceLock::set`), so the
/// first test to run wins for the whole process. Tests that need to
/// assert the no-token-installed path use the bare `subprocess_origin`
/// without going through this helper.
fn install_test_token() -> &'static str {
    init_agent_origin_token("subprocess-test-token".to_string());
    agent_origin_token().expect("token installed")
}

#[test]
fn subprocess_origin_not_subprocess_with_no_token_header() {
    install_test_token();
    let h = headers_with(&[("user-agent", "curl/8.7.1")]);
    assert_eq!(subprocess_origin(&h), SubprocessOrigin::NotSubprocess);
}

#[test]
fn subprocess_origin_not_subprocess_with_wrong_token() {
    install_test_token();
    let h = headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, "not-the-real-token")]);
    assert_eq!(subprocess_origin(&h), SubprocessOrigin::NotSubprocess);
}

#[test]
fn subprocess_origin_subprocess_with_matching_token_no_thread() {
    let token = install_test_token();
    let h = headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, token)]);
    assert_eq!(
        subprocess_origin(&h),
        SubprocessOrigin::Subprocess {
            source_thread_id: None
        }
    );
}

#[test]
fn subprocess_origin_subprocess_with_matching_token_and_thread_id() {
    let token = install_test_token();
    let source = Uuid::new_v4();
    let h = headers_with(&[
        (HEADER_AGENT_ORIGIN_TOKEN, token),
        (HEADER_SOURCE_THREAD_ID, &source.to_string()),
    ]);
    assert_eq!(
        subprocess_origin(&h),
        SubprocessOrigin::Subprocess {
            source_thread_id: Some(source)
        }
    );
}

#[test]
fn subprocess_origin_ignores_malformed_thread_id() {
    // Malformed source-thread-id is dropped silently — request is still a
    // subprocess; popover just won't link.
    let token = install_test_token();
    let h = headers_with(&[
        (HEADER_AGENT_ORIGIN_TOKEN, token),
        (HEADER_SOURCE_THREAD_ID, "not-a-uuid"),
    ]);
    assert_eq!(
        subprocess_origin(&h),
        SubprocessOrigin::Subprocess {
            source_thread_id: None
        }
    );
}

#[test]
fn subprocess_origin_env_vars_with_token_and_thread_id_emits_both() {
    install_test_token();
    let tid = Uuid::new_v4();
    let vars = subprocess_origin_env_vars(Some(tid));
    let map: std::collections::HashMap<_, _> = vars.into_iter().collect();
    assert_eq!(
        map.get(ENV_AGENT_ORIGIN_TOKEN).map(String::as_str),
        Some("subprocess-test-token")
    );
    assert_eq!(
        map.get(ENV_SOURCE_THREAD_ID).map(String::as_str),
        Some(tid.to_string().as_str())
    );
}

#[test]
fn subprocess_origin_env_vars_without_thread_id_emits_token_only() {
    install_test_token();
    let vars = subprocess_origin_env_vars(None);
    assert!(vars.iter().any(|(k, _)| *k == ENV_AGENT_ORIGIN_TOKEN));
    assert!(vars.iter().all(|(k, _)| *k != ENV_SOURCE_THREAD_ID));
}

// Shared serialization for tests that mutate process-wide LUCIDOS_API_PORT.
// Per-function `static` items have unique addresses, so each test would
// otherwise get its own mutex and they would race each other.
static API_PORT_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn host_pid_var<'a>(vars: &'a [(&'static str, String)], key: &str) -> Option<&'a str> {
    vars.iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.as_str())
}

#[test]
fn host_protection_env_vars_always_sets_host_pid() {
    let _guard = API_PORT_ENV_LOCK.lock().unwrap();
    let workspace = tempfile::tempdir().expect("tempdir");
    let vars = host_protection_env_vars(workspace.path());
    assert_eq!(
        host_pid_var(&vars, "LUCIDOS_HOST_PID"),
        Some(std::process::id().to_string().as_str()),
        "LUCIDOS_HOST_PID must equal the engine's own pid"
    );
}

#[test]
fn host_protection_env_vars_includes_frontend_pid_when_pidfile_present() {
    let _guard = API_PORT_ENV_LOCK.lock().unwrap();
    let workspace = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(workspace.path().join(".lucidos")).unwrap();
    std::fs::write(workspace.path().join(".lucidos/frontend.pid"), "54321\n").unwrap();
    let vars = host_protection_env_vars(workspace.path());
    assert_eq!(
        host_pid_var(&vars, "LUCIDOS_FRONTEND_PID"),
        Some("54321"),
        "LUCIDOS_FRONTEND_PID must equal the trimmed pidfile contents"
    );
}

#[test]
fn host_protection_env_vars_omits_frontend_pid_when_pidfile_missing() {
    // Tauri / production-bundled installs have no separate Vite process.
    // Absence of frontend.pid → env var absent (not empty string — an
    // empty value would still satisfy `[ -n "$LUCIDOS_FRONTEND_PID" ]`
    // in some shells and confuse the guard).
    let _guard = API_PORT_ENV_LOCK.lock().unwrap();
    let workspace = tempfile::tempdir().expect("tempdir");
    let vars = host_protection_env_vars(workspace.path());
    assert!(
        host_pid_var(&vars, "LUCIDOS_FRONTEND_PID").is_none(),
        "LUCIDOS_FRONTEND_PID must be unset when no pidfile exists"
    );
}

#[test]
fn host_protection_env_vars_omits_frontend_pid_when_pidfile_blank() {
    let _guard = API_PORT_ENV_LOCK.lock().unwrap();
    let workspace = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(workspace.path().join(".lucidos")).unwrap();
    std::fs::write(workspace.path().join(".lucidos/frontend.pid"), "   \n").unwrap();
    let vars = host_protection_env_vars(workspace.path());
    assert!(
        host_pid_var(&vars, "LUCIDOS_FRONTEND_PID").is_none(),
        "LUCIDOS_FRONTEND_PID must be unset when pidfile is blank"
    );
}

#[test]
fn host_protection_env_vars_omits_frontend_pid_when_pidfile_is_not_a_pid() {
    // A multi-line or otherwise junk pidfile would otherwise inject an
    // embedded newline into the env var, silently disabling the hook's
    // regex match for that pid. Reject anything that isn't a clean u32.
    let _guard = API_PORT_ENV_LOCK.lock().unwrap();
    let workspace = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(workspace.path().join(".lucidos")).unwrap();
    std::fs::write(
        workspace.path().join(".lucidos/frontend.pid"),
        "12345\n67890\n",
    )
    .unwrap();
    let vars = host_protection_env_vars(workspace.path());
    assert!(
        host_pid_var(&vars, "LUCIDOS_FRONTEND_PID").is_none(),
        "LUCIDOS_FRONTEND_PID must be unset when pidfile is not a clean u32"
    );
}

#[test]
fn host_protection_env_vars_includes_api_port_when_engine_has_one() {
    // The pre-kill Bash hook uses LUCIDOS_API_PORT to block patterns like
    // `lsof -ti :<port> | xargs kill` that target the engine port.
    let _guard = API_PORT_ENV_LOCK.lock().unwrap();
    let workspace = tempfile::tempdir().expect("tempdir");
    // SAFETY: process-wide env mutation gated by API_PORT_ENV_LOCK.
    unsafe {
        std::env::set_var("LUCIDOS_API_PORT", "3007");
    }
    let vars = host_protection_env_vars(workspace.path());
    unsafe {
        std::env::remove_var("LUCIDOS_API_PORT");
    }
    assert_eq!(host_pid_var(&vars, "LUCIDOS_API_PORT"), Some("3007"),);
}

#[test]
fn host_protection_env_vars_omits_api_port_when_engine_has_none() {
    let _guard = API_PORT_ENV_LOCK.lock().unwrap();
    let workspace = tempfile::tempdir().expect("tempdir");
    unsafe {
        std::env::remove_var("LUCIDOS_API_PORT");
    }
    let vars = host_protection_env_vars(workspace.path());
    assert!(
        host_pid_var(&vars, "LUCIDOS_API_PORT").is_none(),
        "LUCIDOS_API_PORT must be unset when engine has no port (Tauri / tests)"
    );
}

#[test]
fn host_protection_env_vars_adds_loopback_base_url_under_gateway() {
    // Under the workspace gateway (ADR 0013) the engine binds a LOOPBACK port
    // and serves plain HTTP — the gateway terminates TLS and routes the
    // workspace under `/ws/<id>/`. A subprocess that builds its URL from
    // `.lucidos/ports` (the gateway port, no `/ws/<id>/` prefix) gets the
    // picker's SPA HTML back instead of JSON. The engine hands same-host
    // subprocesses the exact loopback base URL so the `lucidos` CLI and scripts
    // reach the engine directly.
    let _guard = API_PORT_ENV_LOCK.lock().unwrap();
    let workspace = tempfile::tempdir().expect("tempdir");
    // SAFETY: process-wide env mutation gated by API_PORT_ENV_LOCK.
    unsafe {
        std::env::set_var("LUCIDOS_API_PORT", "62072");
        std::env::set_var("LUCIDOS_BIND_LOOPBACK", "1");
    }
    let vars = host_protection_env_vars(workspace.path());
    unsafe {
        std::env::remove_var("LUCIDOS_API_PORT");
        std::env::remove_var("LUCIDOS_BIND_LOOPBACK");
    }
    // The scheme comes from `net_config::tls_scheme()` (what THIS process
    // serves), which reads ambient `LUCIDOS_TLS_*` — a dev-spawned test shell
    // carries them, a fronted engine has them stripped. Assert through the same
    // resolver so the test is env-agnostic; the scheme decision itself is
    // covered by net_config's own tests.
    let expected = format!("{}://127.0.0.1:62072", crate::net_config::tls_scheme());
    assert_eq!(
        host_pid_var(&vars, "LUCIDOS_API_BASE_URL"),
        Some(expected.as_str()),
        "loopback engine base URL must be handed to subprocesses under the gateway"
    );
}

#[test]
fn host_protection_env_vars_omits_base_url_without_gateway() {
    // Legacy / Tauri / production: the engine listens on the user-facing port
    // and the `.lucidos/ports` file resolves it correctly, so no override is
    // handed down. `LUCIDOS_BIND_LOOPBACK` is set ONLY by the gateway.
    let _guard = API_PORT_ENV_LOCK.lock().unwrap();
    let workspace = tempfile::tempdir().expect("tempdir");
    unsafe {
        std::env::set_var("LUCIDOS_API_PORT", "5173");
        std::env::remove_var("LUCIDOS_BIND_LOOPBACK");
    }
    let vars = host_protection_env_vars(workspace.path());
    unsafe {
        std::env::remove_var("LUCIDOS_API_PORT");
    }
    assert!(
        host_pid_var(&vars, "LUCIDOS_API_BASE_URL").is_none(),
        "no base-URL override outside the gateway (the ports file resolves the engine)"
    );
}

#[test]
fn build_origin_subprocess_overrides_human_to_agent_api() {
    // The actual incident: agent shells out `curl -X POST /api/v1/changes/X/apply`.
    // No body, no `mode` field — handler defaults to ActorMode::Human.
    // With the subprocess token in headers, the resulting actor MUST
    // be stamped as Api { mode: Agent, source_thread_id } instead of
    // bleeding through as Api { mode: Human } (which the UI renders as "You").
    let token = install_test_token();
    let source = Uuid::new_v4();
    let h = headers_with(&[
        ("user-agent", "curl/8.7.1"),
        (HEADER_AGENT_ORIGIN_TOKEN, token),
        (HEADER_SOURCE_THREAD_ID, &source.to_string()),
    ]);
    let origin = build_message_origin(&h, ActorMode::Human, None, None, None, None, None, None);
    match origin {
        Some(MessageOrigin::Api {
            user_agent,
            mode,
            source_thread_id,
        }) => {
            assert_eq!(user_agent.as_deref(), Some("curl/8.7.1"));
            assert_eq!(
                mode,
                ActorMode::Agent,
                "subprocess origin must downgrade Human → Agent so the UI \
                 doesn't render an agent action as a 'You' card"
            );
            assert_eq!(
                source_thread_id,
                Some(source),
                "spawning thread id must flow through for popover attribution"
            );
        }
        other => panic!(
            "expected Api{{mode: Agent, source_thread_id: Some(_)}}, got {:?}",
            other
        ),
    }
}

#[test]
fn build_origin_subprocess_overrides_device_id_too() {
    // A subprocess could also send the device-id header (the run_bash
    // shell inherits engine env; the agent could read it). Subprocess
    // origin still wins — agent actions never appear as "You".
    let token = install_test_token();
    let h = headers_with(&[
        ("user-agent", "curl/8.7.1"),
        (HEADER_AGENT_ORIGIN_TOKEN, token),
        ("x-lucidos-device-id", "should-be-ignored"),
    ]);
    let origin = build_message_origin(
        &h,
        ActorMode::Human,
        Some("should-be-ignored"),
        Some("Chrome on Mac".into()),
        None,
        None,
        None,
        None,
    );
    assert!(
        matches!(
            origin,
            Some(MessageOrigin::Api {
                mode: ActorMode::Agent,
                ..
            })
        ),
        "subprocess origin must beat a Device id, got {:?}",
        origin
    );
}

#[test]
fn build_origin_cross_workspace_caller_still_wins_over_subprocess() {
    // Cross-workspace calls go through their own resolution (Workspace
    // origin); a subprocess token presented on the SAME request would be
    // confusing but the cross-workspace contract is "caller wins".
    let token = install_test_token();
    let h = headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, token)]);
    let origin = build_message_origin(
        &h,
        ActorMode::Human,
        None,
        None,
        None,
        None,
        None,
        caller("dev", None, None, ActorMode::Human),
    );
    assert!(
        matches!(origin, Some(MessageOrigin::Workspace { .. })),
        "caller_workspace beats subprocess token (cross-workspace path is \
         documented as authoritative), got {:?}",
        origin
    );
}

#[tokio::test]
async fn user_actor_resolved_subprocess_token_overrides_device_lookup() {
    // Regression: even when the request has a device-id header that
    // resolves cleanly to a real row in the devices table, a valid
    // subprocess token MUST take precedence and produce an Api{Agent}
    // actor. The whole point is "agent actions are never 'You', no
    // matter what other identity the request carries".
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    crate::test_support::seed_device(&pool, "real-device", Some("Mozilla"), Some("My MacBook"))
        .await;
    let token = install_test_token();
    let source = Uuid::new_v4();
    let h = headers_with(&[
        ("user-agent", "curl/8.7.1"),
        (HEADER_DEVICE_ID, "real-device"),
        (HEADER_AGENT_ORIGIN_TOKEN, token),
        (HEADER_SOURCE_THREAD_ID, &source.to_string()),
    ]);
    let actor = user_actor_resolved(&h, &pool, None).await;
    match actor {
        Some(MessageOrigin::Api {
            mode,
            source_thread_id,
            ..
        }) => {
            assert_eq!(mode, ActorMode::Agent);
            assert_eq!(source_thread_id, Some(source));
        }
        other => panic!(
            "subprocess token must beat resolved Device row, got {:?}",
            other
        ),
    }
    crate::test_support::teardown_test_db(&db_name).await;
}
