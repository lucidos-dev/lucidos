//! A short-lived app-server asks an ephemeral fork, never the live rollout.

use super::agent_runtime::{AgentEvent, SpawnArgs};
use super::claude_code::{tear_down_cold_child, ColdProcessGroup, SessionCopyReply};
use super::codex_app_server_parse::{parse_app_server_line, AppServerLine, AppServerTracker};
use crate::engine::agent_session::side_question::{
    kept_reaching_for_tools, side_question_timeout_message, SIDE_QUESTION_INSTRUCTIONS,
    SIDE_QUESTION_MAX_TURNS, SIDE_QUESTION_TOOL_REFUSAL,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

type Error = Box<dyn std::error::Error + Send + Sync>;

// Codex scopes hook trust to this exact normalized declaration in session flags.
pub(crate) const TOOL_GUARD_COMMAND: &str = r#"printf '%s\n' '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"Side questions cannot use tools."}}'"#;

fn tool_guard() -> Value {
    json!({"type":"command", "command":TOOL_GUARD_COMMAND, "async":false, "timeout":5})
}

fn guard_config() -> Value {
    let hook = tool_guard();
    // Codex hashes sorted JSON after TOML normalization; optional nulls are absent.
    let mut identity = json!({"event_name":"pre_tool_use", "matcher":"*", "hooks":[hook.clone()]});
    identity.sort_all_objects();
    let hash = format!(
        "sha256:{:x}",
        Sha256::digest(identity.to_string().as_bytes())
    );
    json!({"mcp_servers":{}, "web_search":"disabled",
        "features":{"hooks":true,"shell_tool":false,"apps":false,"plugins":false,
            "plugin_hooks":false,"image_generation":false,"browser_use":false,
            "computer_use":false},
        "hooks":{"PreToolUse":[{"matcher":"*","hooks":[hook]}],
            "state":{"/<session-flags>/config.toml:pre_tool_use:0:0":{
                "enabled":true,"trusted_hash":hash}}}})
}

fn verify_tool_guard(result: &Value) -> Result<(), Error> {
    let valid = result["data"].as_array().is_some_and(|entries| {
        !entries.is_empty()
            && entries
                .iter()
                .all(|entry| entry["errors"].as_array().is_some_and(Vec::is_empty))
            && entries.iter().any(|entry| {
                entry["hooks"].as_array().is_some_and(|hooks| {
                    hooks.iter().any(|hook| {
                        hook["eventName"] == "preToolUse"
                            && hook["handlerType"] == "command"
                            && hook["command"] == TOOL_GUARD_COMMAND
                            && hook["matcher"] == "*"
                            && hook["async"] == false
                            && hook["enabled"] == true
                            && matches!(hook["trustStatus"].as_str(), Some("trusted" | "managed"))
                    })
                })
            })
    });
    if valid {
        Ok(())
    } else {
        Err("Codex cannot verify the side-question tool guard. Update Codex and try again.".into())
    }
}

fn fork_params(session: &str, cwd: &std::path::Path, model: Option<&str>) -> Value {
    json!({"threadId":session, "ephemeral":true, "excludeTurns":true,
        "cwd":cwd, "model":model.filter(|m| *m != "default"),
        "sandbox":"read-only", "approvalPolicy":"never",
        "config":guard_config()})
}

fn is_tool_item(item: &Value) -> bool {
    !matches!(
        item["type"].as_str(),
        Some("agentMessage" | "userMessage" | "reasoning" | "plan" | "contextCompaction")
    )
}

async fn send(out: &mut (impl tokio::io::AsyncWrite + Unpin), value: Value) -> Result<(), Error> {
    out.write_all(format!("{value}\n").as_bytes()).await?;
    out.flush().await?;
    Ok(())
}

async fn request(
    out: &mut (impl tokio::io::AsyncWrite + Unpin),
    id: u64,
    method: &str,
    params: Value,
) -> Result<(), Error> {
    send(
        out,
        json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}),
    )
    .await
}

/// The fork uses the same executable, environment and outer sandbox as its main session.
/// No protocol switch can select a persistent resume for this path.
pub(crate) async fn ask_side_question(
    args: SpawnArgs<'_>,
    question: &str,
    image_paths: &[std::path::PathBuf],
    deadline: tokio::time::Instant,
) -> Result<SessionCopyReply, Error> {
    let session = args
        .resume_session_id
        .ok_or("This thread has no Codex session to ask yet")?;
    if let Some(path) = args.binary_override {
        super::spawn_env::resolve_binary_override(
            path,
            "Codex (`codex`)",
            crate::core::prefs::CODING_AGENT_CODEX_PATH.key(),
        )?;
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let binary = super::codex::resolve_codex_binary(
        home.as_deref(),
        args.binary_override.map(std::path::Path::new),
    );
    let mut cmd = args.sandbox.command(&binary)?;
    cmd.arg("app-server")
        .current_dir(args.worktree_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let (env, removed) = super::codex::baked_lucidos_env(
        &args,
        super::lucidos_cli::lucidos_cli_dir(),
        &args.sandbox,
    );
    for key in removed {
        cmd.env_remove(key);
    }
    for (key, value) in env {
        cmd.env(key, value);
    }
    let secrets = super::spawn_env::command_secret_values(&cmd);
    let child = super::spawn_env::spawn_below_engine(&mut cmd).map_err(|error| {
        format!("Could not start Codex at {}: {error}. Install Codex or set its executable path in Settings.", binary.to_string_lossy())
    })?;
    ask_child(
        child,
        fork_params(session, args.worktree_path, args.model),
        question,
        image_paths,
        args.model,
        args.reasoning_effort,
        secrets,
        deadline,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn ask_child(
    mut child: tokio::process::Child,
    fork: Value,
    question: &str,
    images: &[std::path::PathBuf],
    model: Option<&str>,
    effort: Option<&str>,
    secrets: Vec<String>,
    deadline: tokio::time::Instant,
) -> Result<SessionCopyReply, Error> {
    let group = ColdProcessGroup(child.id());
    let stderr = child
        .stderr
        .take()
        .map(|pipe| super::spawn_env::StderrTail::spawn(pipe, secrets));
    let mut input = child.stdin.take().ok_or("Codex stdin missing")?;
    let mut lines = BufReader::new(child.stdout.take().ok_or("Codex stdout missing")?).lines();
    let mut resolved_model = model.map(str::to_owned);
    let mut usage = crate::engine::ApiUsage::default();
    let mut seen_usage = false;
    let exchange = async {
        request(&mut input, 1, "initialize", json!({"clientInfo":{"name":"lucidos-side-question","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}})).await?;
        let mut thread = String::new();
        let mut tracker = AppServerTracker::default();
        let mut turns = 0;
        let mut refused = false;
        let mut tool_completed = false;
        let mut turn_id = String::new();
        loop {
            let line = lines
                .next_line()
                .await?
                .ok_or("Codex exited before answering the side question")?;
            match parse_app_server_line(&line) {
                AppServerLine::Response {
                    id: 4,
                    error: Some(error),
                    ..
                } => {
                    log!("[CodexSideQuestion] turn/interrupt rejected: {}", error);
                }
                AppServerLine::Response {
                    id,
                    error: Some(error),
                    ..
                } => {
                    let message = if matches!(id, 2 | 5 | 6) {
                        format!("Codex could not create an ephemeral side-question fork. Update Codex and try again. {error}")
                    } else {
                        format!("Codex side question failed: {error}")
                    };
                    return Err::<SessionCopyReply, Error>(message.into());
                }
                AppServerLine::Response { id: 1, .. } => {
                    send(&mut input, json!({"jsonrpc":"2.0","method":"initialized"})).await?;
                    request(&mut input, 5, "config/read", json!({"cwd": fork["cwd"]})).await?;
                }
                AppServerLine::Response { id: 5, result, .. } => {
                    let mut fork = fork.clone();
                    // Empty tables merge with inherited config; each server needs an explicit disable.
                    let servers = result["config"]["mcp_servers"]
                        .as_object()
                        .ok_or("Codex returned no MCP configuration; update Codex and try again")?;
                    fork["config"]["mcp_servers"] = Value::Object(
                        servers
                            .keys()
                            .map(|name| (name.clone(), json!({"enabled":false})))
                            .collect(),
                    );
                    request(&mut input, 2, "thread/fork", fork).await?;
                }
                AppServerLine::Response { id: 2, result, .. } => {
                    thread = result["thread"]["id"]
                        .as_str()
                        .ok_or("Codex fork returned no thread id")?
                        .to_owned();
                    if thread == fork["threadId"].as_str().unwrap_or_default() {
                        return Err("Codex returned the original session instead of a fork".into());
                    }
                    resolved_model = result["model"]
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| resolved_model.clone());
                    request(&mut input, 6, "hooks/list", json!({"threadId":thread})).await?;
                }
                AppServerLine::Response { id: 6, result, .. } => {
                    verify_tool_guard(&result)?;
                    tracker = AppServerTracker::new(Some(thread.clone()));
                    tracker.begin_turn(effort.map(str::to_owned));
                    turns += 1;
                    let text = format!("{SIDE_QUESTION_INSTRUCTIONS}\n\n{question}");
                    request(
                        &mut input,
                        3,
                        "turn/start",
                        super::codex_app_server::build_turn_start_params(
                            &thread, &text, images, model, effort,
                        ),
                    )
                    .await?;
                }
                AppServerLine::Response { id: 3, result, .. } => {
                    turn_id = result["turn"]["id"].as_str().unwrap_or_default().to_owned();
                }
                AppServerLine::ServerRequest { id, method, .. } => {
                    if method.ends_with("/requestApproval") {
                        send(&mut input, json!({"id":id,"result":{"decision":"decline"}})).await?;
                    } else {
                        send(&mut input, json!({"id":id,"error":{"code":-32601,"message":SIDE_QUESTION_TOOL_REFUSAL}})).await?;
                    }
                    if !refused {
                        refused = true;
                        request(
                            &mut input,
                            4,
                            "turn/interrupt",
                            json!({"threadId":thread,"turnId":turn_id}),
                        )
                        .await?;
                    }
                }
                AppServerLine::Notification { method, params } => {
                    if params["threadId"].as_str().is_some_and(|id| id != thread) {
                        continue;
                    }
                    if method == "turn/started" {
                        turn_id = params["turn"]["id"].as_str().unwrap_or_default().to_owned();
                    }
                    if method == "item/completed" && is_tool_item(&params["item"]) {
                        tool_completed = true;
                    }
                    if matches!(method.as_str(), "item/started" | "item/completed")
                        && is_tool_item(&params["item"])
                        && !refused
                    {
                        refused = true;
                        request(
                            &mut input,
                            4,
                            "turn/interrupt",
                            json!({"threadId":thread,"turnId":turn_id}),
                        )
                        .await?;
                    }
                    for event in tracker.map_notification(&method, &params, 0) {
                        if let AgentEvent::Usage {
                            input_tokens,
                            output_tokens,
                            cache_read_tokens,
                            cache_creation_tokens,
                            ..
                        } = event
                        {
                            seen_usage = true;
                            usage.input_tokens = usage.input_tokens.saturating_add(
                                input_tokens
                                    .saturating_add(cache_read_tokens)
                                    .saturating_add(cache_creation_tokens),
                            );
                            usage.output_tokens = usage.output_tokens.saturating_add(output_tokens);
                            usage.cache_read_tokens =
                                usage.cache_read_tokens.saturating_add(cache_read_tokens);
                            usage.cache_creation_tokens = usage
                                .cache_creation_tokens
                                .saturating_add(cache_creation_tokens);
                        }
                    }
                    if method == "turn/completed" {
                        if tool_completed || !refused || turns >= SIDE_QUESTION_MAX_TURNS {
                            let answer = if tool_completed {
                                Err("Codex completed a tool during a side question; no answer was accepted. Update Codex and try again.".into())
                            } else if refused {
                                Err(kept_reaching_for_tools("Codex"))
                            } else if params["turn"]["status"] != "completed" {
                                Err(format!("Codex side question failed: {}", params["turn"]))
                            } else {
                                let text = tracker.turn_text();
                                if text.trim().is_empty() {
                                    Err("Codex returned no answer".into())
                                } else {
                                    Ok(text)
                                }
                            };
                            return Ok(SessionCopyReply {
                                answer,
                                usage: seen_usage.then_some(usage),
                                model: resolved_model.clone(),
                            });
                        }
                        turns += 1;
                        refused = false;
                        tracker.begin_turn(effort.map(str::to_owned));
                        request(
                            &mut input,
                            3,
                            "turn/start",
                            super::codex_app_server::build_turn_start_params(
                                &thread,
                                SIDE_QUESTION_TOOL_REFUSAL,
                                &[],
                                model,
                                effort,
                            ),
                        )
                        .await?;
                    }
                }
                _ => {}
            }
        }
    };
    let result = tokio::time::timeout_at(deadline, exchange)
        .await
        .unwrap_or_else(|_| Err(side_question_timeout_message("Codex").into()));
    tear_down_cold_child(group, &mut child, "Codex side question").await;
    let stderr_text = match stderr {
        Some(stderr) => stderr.finish().await,
        None => String::new(),
    };
    let result = result.map_err(|error| -> Error {
        if stderr_text.is_empty() {
            error
        } else {
            format!("{error}: {stderr_text}").into()
        }
    });
    match result {
        Err(error) if seen_usage => Ok(SessionCopyReply {
            answer: Err(error.to_string()),
            usage: Some(usage),
            model: resolved_model,
        }),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn stub(
        body: &str,
        timeout: std::time::Duration,
    ) -> (Result<SessionCopyReply, Error>, String) {
        stub_with_config(
            body,
            timeout,
            json!({"id":5,"result":{"config":{"mcp_servers":{"inherited":{"enabled":true}}}}}),
        )
        .await
    }

    async fn stub_with_config(
        body: &str,
        timeout: std::time::Duration,
        config_reply: Value,
    ) -> (Result<SessionCopyReply, Error>, String) {
        stub_with_guard(body, timeout, config_reply, valid_guard()).await
    }

    fn valid_guard() -> Value {
        json!({"id":6,"result":{"data":[{"errors":[],"hooks":[{
            "eventName":"preToolUse","command":TOOL_GUARD_COMMAND,"matcher":"*",
            "handlerType":"command","async":false,"enabled":true,"trustStatus":"trusted"
        }]}]}})
    }

    async fn stub_with_guard(
        body: &str,
        timeout: std::time::Duration,
        config_reply: Value,
        guard_reply: Value,
    ) -> (Result<SessionCopyReply, Error>, String) {
        let guard_reply = guard_reply.to_string().replace('\'', "'\"'\"'");
        let guard_case =
            format!("*'\"method\":\"hooks/list\"'*) printf '%s\\n' '{guard_reply}' ;;");
        let body = body.replace(
            "  case \"$line\" in",
            &format!("  case \"$line\" in\n    {guard_case}"),
        );
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("requests");
        let script = format!(
            r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$SQ_LOG"
  case "$line" in
    *'"method":"initialize"'*) printf '%s\n' '{{"id":1,"result":{{}}}}' ;;
    {guard_case}
    *'"method":"config/read"'*) printf '%s\n' '{config_reply}' ;;
    *'"method":"thread/fork"'*) {body} ;;
  esac
done
"#
        );
        let mut cmd = super::super::agent_sandbox::Sandbox::unsandboxed()
            .command("/bin/sh")
            .unwrap();
        cmd.args(["-c", &script])
            .env("SQ_LOG", &log)
            .env("OPENAI_API_KEY", "side-question-test-secret")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let secrets = super::super::spawn_env::command_secret_values(&cmd);
        let child = super::super::spawn_env::spawn_below_engine(&mut cmd).unwrap();
        let result = ask_child(
            child,
            fork_params("main", dir.path(), None),
            "question",
            &[],
            None,
            None,
            secrets,
            tokio::time::Instant::now() + timeout,
        )
        .await;
        (result, std::fs::read_to_string(log).unwrap_or_default())
    }

    #[tokio::test]
    async fn old_cli_and_child_exit_settle() {
        let (error, log) = stub_with_config(
            "break",
            std::time::Duration::from_secs(3),
            json!({"id":5,"error":{"code":-32600,"message":"unknown variant config/read"}}),
        )
        .await;
        assert!(error.unwrap_err().to_string().contains("Update Codex"));
        assert!(!log.contains("thread/fork"));
        let (error, _) = stub(r#"printf '%s\n' '{"id":2,"error":{"code":-32600,"message":"unknown variant thread/fork"}}'"#, std::time::Duration::from_secs(3)).await;
        assert!(error.unwrap_err().to_string().contains("Update Codex"));
        let (error, _) = stub("break", std::time::Duration::from_secs(3)).await;
        assert!(error.unwrap_err().to_string().contains("exited"));
    }

    #[tokio::test]
    async fn child_exit_reports_masked_stderr() {
        let (error, _) = stub(
            r#"printf '%s\n' "invalid config: $OPENAI_API_KEY" >&2; break"#,
            std::time::Duration::from_secs(3),
        )
        .await;
        let message = error.unwrap_err().to_string();
        assert!(message.contains("invalid config"), "{message}");
        assert!(!message.contains("side-question-test-secret"), "{message}");
    }

    #[tokio::test]
    async fn deadline_settles_a_silent_child() {
        let (error, _) = stub(":", std::time::Duration::from_millis(100)).await;
        assert!(error.unwrap_err().to_string().contains("did not answer"));
    }

    #[tokio::test]
    async fn answer_reports_resolved_model_and_normalized_usage() {
        let body = r#"
printf '%s\n' '{"id":2,"result":{"thread":{"id":"fork"},"model":"resolved"}}'
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$SQ_LOG"
  case "$line" in
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":3,"result":{"turn":{"id":"turn"}}}'
      printf '%s\n' '{"method":"item/started","params":{"threadId":"fork","item":{"type":"contextCompaction","id":"compact"}}}'
      printf '%s\n' '{"method":"item/completed","params":{"threadId":"fork","item":{"type":"plan","id":"plan","text":"Answer directly"}}}'
      printf '%s\n' '{"method":"item/completed","params":{"threadId":"fork","item":{"type":"agentMessage","id":"answer","text":"Forty two"}}}'
      printf '%s\n' '{"method":"thread/tokenUsage/updated","params":{"threadId":"fork","tokenUsage":{"last":{"inputTokens":100,"cachedInputTokens":30,"cacheWriteInputTokens":10,"outputTokens":5}}}}'
      printf '%s\n' '{"method":"thread/tokenUsage/updated","params":{"threadId":"fork","tokenUsage":{"last":{"inputTokens":25,"cachedInputTokens":5,"cacheWriteInputTokens":2,"outputTokens":3}}}}'
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"fork","turn":{"id":"turn","status":"completed"}}}' ;;
  esac
done
"#;
        let (reply, log) = stub(body, std::time::Duration::from_secs(3)).await;
        let reply = reply.unwrap();
        assert_eq!(reply.answer.unwrap(), "Forty two");
        assert_eq!(reply.model.as_deref(), Some("resolved"));
        let usage = reply.usage.unwrap();
        assert_eq!(
            (
                usage.input_tokens,
                usage.cache_read_tokens,
                usage.cache_creation_tokens,
                usage.output_tokens
            ),
            (125, 35, 12, 8)
        );
        assert!(!log.contains("turn/interrupt"));
        assert!(log.contains("thread/fork"));
        assert!(!log.contains("thread/resume"));
        let fork: Value = log
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .find(|value| value["method"] == "thread/fork")
            .unwrap();
        assert_eq!(
            fork["params"]["config"]["mcp_servers"]["inherited"]["enabled"],
            false
        );
    }

    #[tokio::test]
    async fn usage_survives_an_exchange_error() {
        let body = r#"
printf '%s\n' '{"id":2,"result":{"thread":{"id":"fork"},"model":"resolved"}}'
while IFS= read -r line; do
  case "$line" in
    *'"method":"turn/start"'*)
      printf '%s\n' '{"method":"thread/tokenUsage/updated","params":{"threadId":"fork","tokenUsage":{"last":{"inputTokens":100,"cachedInputTokens":30,"outputTokens":5}}}}'
      break ;;
  esac
done
break
"#;
        let (reply, _) = stub(body, std::time::Duration::from_secs(3)).await;
        let reply = reply.unwrap();
        assert!(reply.answer.unwrap_err().contains("exited"));
        assert_eq!(reply.model.as_deref(), Some("resolved"));
        let usage = reply.usage.unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.cache_read_tokens, 30);
        assert_eq!(usage.output_tokens, 5);
    }

    #[tokio::test]
    async fn tools_are_declined_interrupted_and_capped() {
        let body = r#"
printf '%s\n' '{"id":2,"result":{"thread":{"id":"fork"}}}'
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$SQ_LOG"
  case "$line" in
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":3,"result":{"turn":{"id":"turn"}}}'
      printf '%s\n' '{"method":"item/started","params":{"threadId":"fork","item":{"type":"commandExecution","id":"tool"}}}'
      printf '%s\n' '{"id":"approval","method":"item/commandExecution/requestApproval","params":{}}' ;;
    *'"method":"turn/interrupt"'*)
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"fork","turn":{"id":"turn","status":"interrupted"}}}'
      printf '%s\n' '{"id":4,"error":{"code":-32600,"message":"turn already finished"}}' ;;
  esac
done
"#;
        let (reply, log) = stub(body, std::time::Duration::from_secs(3)).await;
        assert_eq!(
            reply.unwrap().answer.unwrap_err(),
            kept_reaching_for_tools("Codex")
        );
        assert_eq!(log.matches("turn/start").count(), SIDE_QUESTION_MAX_TURNS);
        assert!(log.contains("decline"));
        assert!(log.contains(SIDE_QUESTION_TOOL_REFUSAL));
        assert!(!log.contains("accept"));
    }

    #[tokio::test]
    async fn unverifiable_guard_never_starts_a_turn() {
        let mut invalid = vec![
            json!({"id":6,"error":{"message":"unknown hooks/list"}}),
            json!({"id":6,"result":{}}),
            json!({"id":6,"result":{"data":[]}}),
        ];
        for (field, value) in [
            ("enabled", json!(false)),
            ("async", json!(true)),
            ("trustStatus", json!("untrusted")),
            ("command", json!("echo allow")),
            ("matcher", json!("Bash")),
            ("handlerType", json!("prompt")),
            ("eventName", json!("postToolUse")),
        ] {
            let mut reply = valid_guard();
            reply["result"]["data"][0]["hooks"][0][field] = value;
            invalid.push(reply);
        }
        let mut errors = valid_guard();
        errors["result"]["data"][0]["errors"] = json!([{"message":"bad hook"}]);
        invalid.push(errors);
        for reply in invalid {
            let (result, log) = stub_with_guard(
                r#"printf '%s\n' '{"id":2,"result":{"thread":{"id":"fork"}}}'"#,
                std::time::Duration::from_secs(3),
                json!({"id":5,"result":{"config":{"mcp_servers":{}}}}),
                reply,
            )
            .await;
            assert!(result.unwrap_err().to_string().contains("Update Codex"));
            assert!(!log.contains("turn/start"), "{log}");
        }
    }

    #[tokio::test]
    async fn completed_tool_prevents_retry_and_preserves_usage() {
        let body = r#"
printf '%s\n' '{"id":2,"result":{"thread":{"id":"fork"}}}'
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$SQ_LOG"
  case "$line" in
    *'"method":"turn/start"'*)
      printf '%s\n' '{"method":"item/completed","params":{"threadId":"fork","item":{"type":"commandExecution","id":"fast","aggregatedOutput":"secret"}}}'
      printf '%s\n' '{"method":"thread/tokenUsage/updated","params":{"threadId":"fork","tokenUsage":{"last":{"inputTokens":10,"outputTokens":2}}}}' ;;
    *'"method":"turn/interrupt"'*)
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"fork","turn":{"id":"turn","status":"interrupted"}}}' ;;
  esac
done
"#;
        let (reply, log) = stub(body, std::time::Duration::from_secs(3)).await;
        let reply = reply.unwrap();
        assert!(reply.answer.unwrap_err().contains("completed a tool"));
        assert_eq!(reply.usage.unwrap().input_tokens, 10);
        assert_eq!(log.matches("turn/start").count(), 1);
    }

    #[test]
    fn fork_is_ephemeral_and_confined() {
        let request = fork_params("main", std::path::Path::new("/project"), Some("model"));
        assert_eq!(request["threadId"], "main");
        assert_eq!(request["ephemeral"], true);
        assert_eq!(request["excludeTurns"], true);
        assert_eq!(request["sandbox"], "read-only");
        assert_eq!(request["approvalPolicy"], "never");
        assert_eq!(request["config"]["mcp_servers"], json!({}));
    }

    #[test]
    fn only_conversation_items_are_allowed() {
        for kind in [
            "commandExecution",
            "fileChange",
            "mcpToolCall",
            "webSearch",
            "futureTool",
        ] {
            assert!(is_tool_item(&json!({"type": kind})));
        }
        for kind in [
            "agentMessage",
            "userMessage",
            "reasoning",
            "plan",
            "contextCompaction",
        ] {
            assert!(!is_tool_item(&json!({"type": kind})));
        }
    }
}
