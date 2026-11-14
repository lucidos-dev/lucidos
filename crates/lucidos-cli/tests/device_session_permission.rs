//! A device-session refusal prepares an owner permission, prints no credential,
//! and leaves the caller's private output file untouched.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_device_session_permission_request_exits_two_with_answer_instructions() {
    let app = axum::Router::new().route(
        "/api/v1/workspaces/demo/device-session",
        axum::routing::post(
            |axum::Json(body): axum::Json<serde_json::Value>| async move {
                assert_eq!(body["device_id"], "page");
                assert_eq!(body["ttl_secs"], 120);
                (
                    axum::http::StatusCode::FORBIDDEN,
                    axum::Json(serde_json::json!({
                        "reason": "permission_required",
                        "request_id": "permission-request:fixture",
                        "error": "This act needs the owner's permission.",
                        "session": "must-not-print"
                    })),
                )
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let workspace = tempfile::tempdir().unwrap();
    let out = workspace.path().join("session");
    std::fs::write(&out, "existing-private-output").unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_lucidos"))
        .args([
            "workspaces",
            "device-session",
            "demo",
            "--device",
            "page",
            "--ttl",
            "2",
            "--reason",
            "Drive a fixture.",
            "--out",
        ])
        .arg(&out)
        .env("LUCIDOS_WORKSPACE", workspace.path())
        .env("LUCIDOS_API_BASE_URL", format!("http://{address}"))
        .env_remove("LUCIDOS_AGENT_ORIGIN_TOKEN")
        .env_remove("LUCIDOS_THREAD_ID")
        .env_remove("LUCIDOS_EVENT_ID")
        .env_remove("LUCIDOS_REPO")
        .current_dir(workspace.path())
        .output()
        .unwrap();
    server.abort();
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    for text in [
        "`permission-request:fixture`",
        "ask_user_question",
        "run it again",
        "in this turn",
    ] {
        assert!(stdout.contains(text), "{stdout}");
    }
    assert!(!stdout.contains("must-not-print"));
    assert!(!stdout.contains("owner-approval:"));
    assert_eq!(
        std::fs::read_to_string(out).unwrap(),
        "existing-private-output"
    );
}
