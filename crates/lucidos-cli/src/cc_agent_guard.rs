use std::io::Read;
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Value;

use crate::cc_agent_definitions::{self, AgentDefinition, AgentSources, DEFAULT_AGENT_TYPE};
use crate::workspace::BoxError;

const BACKGROUND_REFUSAL: &str =
    "Lucidos refuses a background subagent. Your turn ending kills it, so its \
     report would never reach you. Call Agent again with `run_in_background: \
     false`. Several Agent calls in one message still run in parallel.";

const REMOTE_REFUSAL: &str =
    "Lucidos refuses a remote subagent. It always runs in the background, and \
     its report would never reach you once your turn ends. Call Agent again \
     without `isolation: \"remote\"`.";

fn definition_refusal(agent_type: &str, setting: &str) -> String {
    format!(
        "Lucidos refuses subagent_type `{agent_type}`. Its definition sets \
         `{setting}`, so it runs in the background whatever the call says, and \
         your turn ending kills it before its report reaches you. Call Agent \
         again with another subagent_type."
    )
}

#[derive(Debug, Deserialize)]
struct HookPayload {
    tool_input: Value,
    cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Switches {
    background_tasks_disabled: bool,
    fork_subagent: bool,
}

impl Switches {
    fn from_env(env: impl Fn(&str) -> Option<String>) -> Self {
        let on = |name: &str| env(name).is_some_and(|v| env_truthy(&v));
        Switches {
            background_tasks_disabled: on("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS"),
            fork_subagent: on("CLAUDE_CODE_FORK_SUBAGENT"),
        }
    }

    fn schema_has_background_flag(self) -> bool {
        !(self.background_tasks_disabled || self.fork_subagent)
    }
}

fn env_truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn requested_agent_type(tool_input: &Value, switches: Switches) -> Option<&str> {
    match tool_input.get("subagent_type").and_then(Value::as_str) {
        Some(agent_type) => Some(agent_type),
        None => (!switches.fork_subagent).then_some(DEFAULT_AGENT_TYPE),
    }
}

pub(crate) fn refusal(
    tool_input: &Value,
    definition: Option<&AgentDefinition>,
    switches: Switches,
) -> Option<String> {
    let isolation = tool_input.get("isolation").and_then(Value::as_str);
    if isolation == Some("remote") {
        return Some(REMOTE_REFUSAL.to_string());
    }
    if let Some(definition) = definition {
        if isolation.is_none() && definition.remote_isolation {
            return Some(definition_refusal(
                &definition.agent_type,
                "isolation: remote",
            ));
        }
        if definition.background && !switches.background_tasks_disabled {
            return Some(definition_refusal(
                &definition.agent_type,
                "background: true",
            ));
        }
    }
    let foreground = match tool_input.get("run_in_background") {
        Some(flag) => flag == &Value::Bool(false),
        None => !switches.schema_has_background_flag(),
    };
    (!foreground).then(|| BACKGROUND_REFUSAL.to_string())
}

pub(crate) fn build_refusal(reason: &str) -> String {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    })
    .to_string()
}

pub(crate) fn run() -> Result<(), BoxError> {
    let mut buf = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
        eprintln!("cc-agent-guard: stdin read failed, allowing: {}", e);
        return Ok(());
    }
    let payload: HookPayload = match serde_json::from_str(&buf) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("cc-agent-guard: payload parse failed, allowing: {}", e);
            return Ok(());
        }
    };
    let switches = Switches::from_env(|name| std::env::var(name).ok());
    let cwd = payload.cwd.or_else(|| std::env::current_dir().ok());
    let definition = requested_agent_type(&payload.tool_input, switches)
        .zip(cwd)
        .and_then(|(agent_type, cwd)| {
            cc_agent_definitions::resolve(agent_type, &AgentSources::from_env(cwd))
        });
    if let Some(reason) = refusal(&payload.tool_input, definition.as_ref(), switches) {
        println!("{}", build_refusal(&reason));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DEFAULT: Switches = Switches {
        background_tasks_disabled: false,
        fork_subagent: false,
    };
    const TASKS_DISABLED: Switches = Switches {
        background_tasks_disabled: true,
        fork_subagent: false,
    };
    const FORK: Switches = Switches {
        background_tasks_disabled: false,
        fork_subagent: true,
    };

    fn definition(agent_type: &str, background: bool, remote_isolation: bool) -> AgentDefinition {
        AgentDefinition {
            agent_type: agent_type.to_string(),
            background,
            remote_isolation,
        }
    }

    fn refused(input: &Value, definition: Option<&AgentDefinition>, switches: Switches) -> bool {
        refusal(input, definition, switches).is_some()
    }

    #[test]
    fn an_agent_call_without_the_flag_is_refused() {
        let input = json!({"description": "Explore", "prompt": "…", "subagent_type": "Explore"});
        assert_eq!(
            refusal(&input, None, DEFAULT).as_deref(),
            Some(BACKGROUND_REFUSAL)
        );
    }

    #[test]
    fn an_explicit_background_call_is_refused() {
        let input = json!({"run_in_background": true});
        assert_eq!(
            refusal(&input, None, DEFAULT).as_deref(),
            Some(BACKGROUND_REFUSAL)
        );
    }

    #[test]
    fn a_foreground_call_passes() {
        assert_eq!(
            refusal(&json!({"run_in_background": false}), None, DEFAULT),
            None
        );
    }

    #[test]
    fn a_string_false_is_not_the_foreground() {
        let input = json!({"run_in_background": "false"});
        assert_eq!(
            refusal(&input, None, DEFAULT).as_deref(),
            Some(BACKGROUND_REFUSAL)
        );
    }

    #[test]
    fn a_missing_flag_passes_when_the_schema_has_none() {
        for switches in [TASKS_DISABLED, FORK] {
            assert_eq!(refusal(&json!({"prompt": "…"}), None, switches), None);
        }
    }

    #[test]
    fn a_remote_subagent_is_refused_even_in_the_foreground() {
        let input = json!({"isolation": "remote", "run_in_background": false});
        assert_eq!(
            refusal(&input, None, DEFAULT).as_deref(),
            Some(REMOTE_REFUSAL)
        );
        assert_eq!(
            refusal(&input, None, TASKS_DISABLED).as_deref(),
            Some(REMOTE_REFUSAL)
        );
    }

    #[test]
    fn a_remote_refusal_never_asks_for_the_background_flag() {
        let far = definition("far", false, true);
        let input = json!({"subagent_type": "far"});
        for reason in [
            REMOTE_REFUSAL.to_string(),
            refusal(&input, Some(&far), TASKS_DISABLED).expect("refused"),
        ] {
            assert!(!reason.contains("run_in_background"), "{reason}");
        }
    }

    #[test]
    fn a_worktree_subagent_in_the_foreground_passes() {
        let input = json!({"isolation": "worktree", "run_in_background": false});
        assert_eq!(refusal(&input, None, DEFAULT), None);
    }

    #[test]
    fn a_background_definition_is_refused_even_in_the_foreground() {
        let input = json!({"subagent_type": "scout", "run_in_background": false});
        let scout = definition("scout", true, false);
        let reason = refusal(&input, Some(&scout), DEFAULT).expect("refused");
        assert!(reason.contains("subagent_type `scout`"), "{reason}");
        assert!(reason.contains("`background: true`"), "{reason}");
        assert!(reason.contains("another subagent_type"), "{reason}");
    }

    #[test]
    fn a_background_definition_is_refused_in_fork_mode() {
        let input = json!({"subagent_type": "scout"});
        assert!(refused(
            &input,
            Some(&definition("scout", true, false)),
            FORK
        ));
    }

    #[test]
    fn a_background_definition_passes_with_background_tasks_disabled() {
        let input = json!({"subagent_type": "scout"});
        let scout = definition("scout", true, false);
        assert_eq!(refusal(&input, Some(&scout), TASKS_DISABLED), None);
    }

    #[test]
    fn a_plain_definition_in_the_foreground_passes() {
        let input = json!({"subagent_type": "scout", "run_in_background": false});
        assert_eq!(
            refusal(&input, Some(&definition("scout", false, false)), DEFAULT),
            None
        );
    }

    #[test]
    fn a_remote_definition_is_refused_unless_the_call_sets_isolation() {
        let far = definition("far", false, true);
        let input = json!({"subagent_type": "far", "run_in_background": false});
        let reason = refusal(&input, Some(&far), TASKS_DISABLED).expect("refused");
        assert!(reason.contains("`isolation: remote`"), "{reason}");

        let input =
            json!({"subagent_type": "far", "isolation": "worktree", "run_in_background": false});
        assert_eq!(refusal(&input, Some(&far), DEFAULT), None);
    }

    #[test]
    fn a_call_without_a_type_launches_the_default_unless_it_forks() {
        let input = json!({"prompt": "…"});
        assert_eq!(
            requested_agent_type(&input, DEFAULT),
            Some(DEFAULT_AGENT_TYPE)
        );
        assert_eq!(
            requested_agent_type(&input, TASKS_DISABLED),
            Some(DEFAULT_AGENT_TYPE)
        );
        assert_eq!(requested_agent_type(&input, FORK), None);
        let input = json!({"subagent_type": "scout"});
        assert_eq!(requested_agent_type(&input, FORK), Some("scout"));
    }

    #[test]
    fn the_switches_read_like_claude_code() {
        let env_with = |set: &'static str, value: &'static str| {
            move |name: &str| (name == set).then(|| value.to_string())
        };
        assert!(Switches::from_env(|_| None).schema_has_background_flag());
        for name in [
            "CLAUDE_CODE_DISABLE_BACKGROUND_TASKS",
            "CLAUDE_CODE_FORK_SUBAGENT",
        ] {
            for on in ["1", " TRUE "] {
                let switches = Switches::from_env(env_with(name, on));
                assert!(!switches.schema_has_background_flag(), "{name}={on}");
            }
            for off in ["0", ""] {
                let switches = Switches::from_env(env_with(name, off));
                assert!(switches.schema_has_background_flag(), "{name}={off}");
            }
        }
        let switches = Switches::from_env(env_with("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS", "1"));
        assert!(switches.background_tasks_disabled && !switches.fork_subagent);
        let switches = Switches::from_env(env_with("CLAUDE_CODE_FORK_SUBAGENT", "1"));
        assert!(switches.fork_subagent && !switches.background_tasks_disabled);
    }

    #[test]
    fn the_refusal_is_a_pretooluse_deny_naming_the_fix() {
        let out: Value =
            serde_json::from_str(&build_refusal(BACKGROUND_REFUSAL)).expect("valid JSON");
        let hook = &out["hookSpecificOutput"];
        assert_eq!(hook["hookEventName"], "PreToolUse");
        assert_eq!(hook["permissionDecision"], "deny");
        let reason = hook["permissionDecisionReason"].as_str().unwrap();
        assert!(reason.contains("`run_in_background: false`"), "{reason}");
    }
}
