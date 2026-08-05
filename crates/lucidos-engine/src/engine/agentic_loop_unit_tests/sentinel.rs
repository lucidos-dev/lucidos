mod sentinel_redaction_tests {
    use super::super::match_sentinel;
    use crate::engine::thread_events::ThreadEvent;
    use crate::engine::tools::credentials::CREDENTIAL_REQUEST_PREFIX;
    use crate::engine::tools::plugins::{
        PLUGIN_INSTALL_REQUEST_PREFIX, PLUGIN_UNINSTALL_REQUEST_PREFIX,
    };

    #[test]
    fn no_sentinel_returns_none() {
        assert!(match_sentinel("plain tool result").is_none());
        assert!(match_sentinel("Error: file not found").is_none());
        assert!(match_sentinel("").is_none());
    }

    #[test]
    fn install_sentinel_extracts_payload_and_redacts() {
        let raw = format!(
            "{PLUGIN_INSTALL_REQUEST_PREFIX}{}",
            r#"{"install_id":"abc","files":["apps/x/index.html"]}"#
        );
        let m = match_sentinel(&raw).expect("install sentinel must match");
        match m.event {
            ThreadEvent::PluginInstallRequested { payload } => {
                assert!(payload.starts_with('{'), "payload must be the JSON object");
                assert!(payload.contains("install_id"));
                assert!(payload.contains("apps/x/index.html"));
            }
            other => panic!("expected PluginInstallRequested, got {:?}", other),
        }
        let redacted = m.redacted_text.expect("install must redact for the LLM");
        // Redacted text MUST NOT leak the JSON to the LLM — that's the bug
        // we're fixing (LLM was parsing `overwrites` and chat-asking the user).
        assert!(
            !redacted.contains('{'),
            "redacted text must not contain the JSON payload: {}",
            redacted
        );
        assert!(
            !redacted.contains(PLUGIN_INSTALL_REQUEST_PREFIX),
            "redacted text must not contain the sentinel prefix: {}",
            redacted
        );
        // Must instruct the LLM to wait, not respond, not chat-ask.
        let lower = redacted.to_lowercase();
        assert!(
            lower.contains("panel") && (lower.contains("confirm") || lower.contains("cancel")),
            "redacted text must explain the panel resolves it: {}",
            redacted
        );
    }

    #[test]
    fn uninstall_sentinel_extracts_payload_and_redacts() {
        let raw = format!(
            "{PLUGIN_UNINSTALL_REQUEST_PREFIX}{}",
            r#"{"uninstall_id":"xyz","plugin_id":"foo","files":["apps/foo/index.html"]}"#
        );
        let m = match_sentinel(&raw).expect("uninstall sentinel must match");
        match m.event {
            ThreadEvent::PluginUninstallRequested { payload } => {
                assert!(payload.contains("uninstall_id"));
                assert!(payload.contains("foo"));
            }
            other => panic!("expected PluginUninstallRequested, got {:?}", other),
        }
        let redacted = m.redacted_text.expect("uninstall must redact for the LLM");
        assert!(!redacted.contains('{'));
        assert!(!redacted.contains(PLUGIN_UNINSTALL_REQUEST_PREFIX));
    }

    #[test]
    fn credential_sentinel_extracts_payload_and_redacts() {
        let raw = format!(
            "{CREDENTIAL_REQUEST_PREFIX}{}",
            r#"{"service":"openai","prompt":"Enter API key","base_url":"https://api.openai.com","auth_type":"api_key"}"#
        );
        let m = match_sentinel(&raw).expect("credential sentinel must match");
        match m.event {
            ThreadEvent::CredentialPromptRequested { payload } => {
                assert!(payload.contains("openai"));
            }
            other => panic!("expected CredentialPromptRequested, got {:?}", other),
        }
        let redacted = m.redacted_text.expect("credential must redact for the LLM");
        assert!(!redacted.contains('{'));
    }

    #[test]
    fn email_confirm_sentinel_emits_event_but_does_not_redact() {
        let raw = "[EMAIL_CONFIRM]{\"to\":[\"a@b\"],\"subject\":\"hi\"}".to_string();
        let m = match_sentinel(&raw).expect("email confirm sentinel must match");
        match m.event {
            ThreadEvent::EmailConfirmRequested { payload } => {
                assert!(payload.contains("a@b"));
            }
            other => panic!("expected EmailConfirmRequested, got {:?}", other),
        }
        assert!(
            m.redacted_text.is_none(),
            "email-confirm must pass through unredacted (its tool description already explains the modal)"
        );
    }

    #[test]
    fn sentinel_without_json_returns_none() {
        // Defensive: sentinel prefix without `{` afterwards should not match
        // (the agentic loop pre-redaction code skipped these too).
        let raw = format!("{PLUGIN_INSTALL_REQUEST_PREFIX}no json here");
        assert!(match_sentinel(&raw).is_none());
    }
}

mod run_tool_with_cancel_tests {
    use super::super::run_tool_with_cancel;
    use crate::engine::tools::ToolOutcome;
    use std::time::{Duration, Instant};
    use tokio_util::sync::CancellationToken;

    /// A tool future that completes normally must produce its own result —
    /// the wrapper passes through when no cancel is signaled.
    #[tokio::test]
    async fn returns_tool_result_when_not_cancelled() {
        let cancel_token = CancellationToken::new();
        let fut = async { Ok::<String, String>("ok".to_string()) };
        assert_eq!(
            run_tool_with_cancel(fut, &cancel_token).await,
            Ok("ok".to_string())
        );
    }

    /// Pre-cancelled token MUST short-circuit even when the inner future is
    /// instantly ready. The `biased` poll order in the wrapper guarantees the
    /// cancel arm wins the race — without `biased`, an already-cancelled token
    /// plus a synchronously-ready tool would still let the tool result win
    /// (tokio::select picks fairly otherwise), letting the loop run one more
    /// iteration past the cancel signal.
    #[tokio::test]
    async fn pre_cancelled_token_short_circuits_even_when_tool_ready() {
        let cancel_token = CancellationToken::new();
        cancel_token.cancel();
        let fut = async { Ok::<String, String>("ok".to_string()) };
        assert_eq!(
            run_tool_with_cancel(fut, &cancel_token).await,
            Err("Error: canceled by user".to_string())
        );
    }

    /// Mid-flight cancel: the tool future is parked on a long sleep when
    /// the cancel signal arrives. The wrapper must observe the cancel within
    /// the polling latency — NOT wait for the tool to finish. This is the
    /// regression contract: a hung `urlopen()` (no timeout) must not block
    /// cancel from completing.
    #[tokio::test]
    async fn cancel_aborts_long_running_tool_quickly() {
        let cancel_token = CancellationToken::new();
        let cancel_clone = cancel_token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_clone.cancel();
        });

        let slow_fut = async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok::<String, String>("should never see this".to_string())
        };

        let start = Instant::now();
        let result = run_tool_with_cancel(slow_fut, &cancel_token).await;
        let elapsed = start.elapsed();

        assert_eq!(result, Err("Error: canceled by user".to_string()));
        assert!(
            elapsed < Duration::from_millis(500),
            "cancel must abort within polling latency (got {:?}); the wrapper is \
             not racing the tool against cancel_token.cancelled() if this trips",
            elapsed
        );
    }

    /// Cancel must surface as `Err` so the agent loop classifies it as a
    /// tool failure and stamps `success: false` on the persisted ToolResult.
    /// This is the typed-Result successor to the legacy "starts_with(\"Error:\")"
    /// classifier — `ToolOutcome::Err(_)` is the contracted failure shape.
    #[tokio::test]
    async fn canceled_result_is_typed_err_for_caller() {
        let cancel_token = CancellationToken::new();
        cancel_token.cancel();
        let fut = async { Ok::<String, String>("ok".to_string()) };
        let result: ToolOutcome = run_tool_with_cancel(fut, &cancel_token).await;
        assert!(
            result.is_err(),
            "cancel must produce a typed Err so the agent loop stamps \
             ToolResult.success=false; got {:?}",
            result
        );
    }
}

/// `until_canceled` is the setup-phase sibling of `run_tool_with_cancel`: same
/// `biased` race, but it yields `None` instead of a typed error because the
/// caller has no paired event to emit, it simply ends the turn. See the chat
/// turn setup in `engine/chat/process/run.rs`.
mod until_canceled_tests {
    use crate::engine::agentic_loop::until_canceled;
    use std::time::{Duration, Instant};
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn passes_the_value_through_when_not_cancelled() {
        let cancel_token = CancellationToken::new();
        let fut = async { 7u32 };
        assert_eq!(until_canceled(&cancel_token, fut).await, Some(7));
    }

    /// The `biased` contract. A Stop that landed while the previous phase was
    /// finishing must end the turn, even though the next phase's future happens
    /// to be instantly ready. Without `biased`, tokio picks fairly and the turn
    /// leaks one more phase of setup past the user's click.
    #[tokio::test]
    async fn pre_cancelled_token_wins_over_an_instantly_ready_future() {
        let cancel_token = CancellationToken::new();
        cancel_token.cancel();
        let fut = async { 7u32 };
        assert_eq!(until_canceled(&cancel_token, fut).await, None);
    }

    /// The regression contract: the whole point is that a Stop is observed
    /// WHILE a long phase is in flight (a classification LLM call, a memory
    /// search on a big thread), not after it returns.
    #[tokio::test]
    async fn cancel_aborts_a_long_phase_without_waiting_for_it() {
        let cancel_token = CancellationToken::new();
        let cancel_clone = cancel_token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_clone.cancel();
        });

        let slow_phase = async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            7u32
        };

        let start = Instant::now();
        let result = until_canceled(&cancel_token, slow_phase).await;
        let elapsed = start.elapsed();

        assert_eq!(result, None);
        assert!(
            elapsed < Duration::from_millis(500),
            "a Stop during turn setup must not wait out the phase (got {:?}); \
             this is the ~30s of \"Canceling…\" the wrapper exists to end",
            elapsed
        );
    }
}
