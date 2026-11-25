//! The runner against real scripts, with no event bus and no database.

use super::*;
use crate::input::PendingEvent;
use lucidos_core::core::agent_sandbox::SANDBOXED_ENV;
use lucidos_core::core::event_hooks::{HOOKS_DIR, HOOK_DEFINITION_FILE};
use lucidos_core::core::{DATA_DIR, LUCIDOS_DATABASE_URL};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Instant;

const SLUG: &str = "gate";
const ALLOW: &str = r#"echo '{"answer": "allow"}'"#;

struct Fixture {
    _dir: tempfile::TempDir,
    workspace: PathBuf,
    python: PythonRuntime,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().canonicalize().unwrap();
    let python = PythonRuntime::new(workspace.clone()).unwrap();
    Fixture {
        _dir: dir,
        workspace,
        python,
    }
}

fn input_with(payload: Value) -> HookInput {
    let pending = PendingEvent {
        event_type: "ChangeProposed".to_string(),
        payload,
    };
    HookInput::masked(&pending, &[])
}

fn input() -> HookInput {
    input_with(json!({"thread_id": "t-1", "summary": "Add a page"}))
}

impl Fixture {
    fn hook_dir(&self) -> PathBuf {
        self.workspace.join(DATA_DIR).join(HOOKS_DIR).join(SLUG)
    }

    /// Write the hook `gate` with `script` holding `body`.
    fn hook(&self, script: &str, body: &str, timeout_secs: Option<u64>) -> HookDefinition {
        std::fs::create_dir_all(self.hook_dir()).unwrap();
        std::fs::write(self.hook_dir().join(script), body).unwrap();
        self.definition(script, timeout_secs)
    }

    fn definition(&self, script: &str, timeout_secs: Option<u64>) -> HookDefinition {
        let toml =
            format!("name = \"Gate\"\nevent_type = \"ChangeProposed\"\nscript = \"{script}\"\n");
        std::fs::create_dir_all(self.hook_dir()).unwrap();
        std::fs::write(self.hook_dir().join(HOOK_DEFINITION_FILE), &toml).unwrap();
        HookDefinition {
            timeout_secs,
            ..toml::from_str(&toml).unwrap()
        }
    }

    async fn run_with(
        &self,
        definition: &HookDefinition,
        input: &HookInput,
        sandbox: &Sandbox,
        env: &[(String, String)],
    ) -> HookAnswer {
        let confinement = Confinement {
            sandbox,
            python: &self.python,
            env,
        };
        run_hook(&self.workspace, SLUG, definition, input, &confinement).await
    }

    async fn run(&self, definition: &HookDefinition) -> HookAnswer {
        self.run_with(definition, &input(), &Sandbox::unsandboxed(), &[])
            .await
    }

    fn read(&self, file: &str) -> String {
        std::fs::read_to_string(self.workspace.join(file)).unwrap()
    }
}

fn refusal(answer: HookAnswer, cause: RefusalCause) -> Refusal {
    match answer {
        HookAnswer::Refuse(refusal) if refusal.cause == cause => refusal,
        other => panic!("expected a {cause:?} refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn a_hook_allows() {
    let hooks = fixture();
    let definition = hooks.hook("check.sh", ALLOW, None);
    assert_eq!(
        hooks.run(&definition).await,
        HookAnswer::Allow { events: Vec::new() }
    );
}

/// The script reads the hook's slug and the pending event on stdin, and runs
/// in the workspace, as a trigger script does.
#[tokio::test]
async fn a_hook_reads_its_input_on_stdin() {
    let hooks = fixture();
    let definition = hooks.hook("check.sh", &format!("cat > input.json\n{ALLOW}\n"), None);
    assert!(matches!(
        hooks.run(&definition).await,
        HookAnswer::Allow { .. }
    ));
    let input: Value = serde_json::from_str(&hooks.read("input.json")).unwrap();
    assert_eq!(
        input,
        json!({
            "hook": SLUG,
            "event_type": "ChangeProposed",
            "payload": {"thread_id": "t-1", "summary": "Add a page"},
        })
    );
}

/// An input larger than a pipe buffer cannot wedge a hook that never reads it.
#[tokio::test]
async fn a_hook_may_ignore_its_input() {
    let hooks = fixture();
    let definition = hooks.hook("check.sh", ALLOW, None);
    let input = input_with(json!({"text": "x".repeat(512 * 1024)}));
    let answer = hooks
        .run_with(&definition, &input, &Sandbox::unsandboxed(), &[])
        .await;
    assert!(matches!(answer, HookAnswer::Allow { .. }), "{answer:?}");
}

#[tokio::test]
async fn a_hook_refuses_with_its_reason() {
    let hooks = fixture();
    let definition = hooks.hook(
        "check.sh",
        r#"echo '{"answer": "refuse", "reason": "No plan approved."}'"#,
        None,
    );
    let refusal = refusal(hooks.run(&definition).await, RefusalCause::Refused);
    assert_eq!(refusal.reason, "No plan approved.");
    assert_eq!(refusal.detail, None);
}

#[tokio::test]
async fn a_hook_asks_for_input() {
    let hooks = fixture();
    let definition = hooks.hook(
        "check.sh",
        r#"echo '{"answer": "needs-input", "argument": "ticket"}'"#,
        None,
    );
    assert_eq!(
        hooks.run(&definition).await,
        HookAnswer::NeedsInput {
            argument: "ticket".to_string()
        }
    );
}

#[tokio::test]
async fn a_hook_returns_domain_events() {
    let hooks = fixture();
    let definition = hooks.hook(
        "check.sh",
        r#"echo '{"answer": "allow", "events": [{"event_type": "TicketLinked", "payload": {"summary": "Linked"}}]}'"#,
        None,
    );
    let HookAnswer::Allow { events } = hooks.run(&definition).await else {
        panic!("expected allow");
    };
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "TicketLinked");
}

/// A hook cannot forge engine progress: an engine event name in an allow
/// turns the whole answer into a refusal.
#[tokio::test]
async fn a_hook_returning_an_engine_event_is_refused() {
    let hooks = fixture();
    let definition = hooks.hook(
        "check.sh",
        r#"echo '{"answer": "allow", "events": [{"event_type": "ChangeApplied"}]}'"#,
        None,
    );
    let refusal = refusal(hooks.run(&definition).await, RefusalCause::InvalidAnswer);
    assert!(
        refusal.reason.contains("domain events only"),
        "{}",
        refusal.reason
    );
    assert!(refusal.reason.contains(SLUG), "{}", refusal.reason);
}

/// A crash refuses, even after the script printed an allow.
#[tokio::test]
async fn a_crash_refuses() {
    let hooks = fixture();
    let definition = hooks.hook(
        "check.sh",
        &format!("{ALLOW}\necho boom >&2\nexit 3\n"),
        None,
    );
    let refusal = refusal(hooks.run(&definition).await, RefusalCause::Crashed);
    assert!(refusal.reason.contains("exit code 3"), "{}", refusal.reason);
    assert!(refusal.reason.contains(SLUG), "{}", refusal.reason);
    assert_eq!(refusal.detail.as_deref(), Some("boom"));
}

#[tokio::test]
async fn output_that_is_no_answer_refuses() {
    let hooks = fixture();
    for body in [
        "echo allowed\n",
        "true\n",
        r#"echo '{"answer": "allow", "reason": "x"}'"#,
    ] {
        let definition = hooks.hook("check.sh", body, None);
        refusal(hooks.run(&definition).await, RefusalCause::InvalidAnswer);
    }
}

#[tokio::test]
async fn an_answer_past_the_cap_refuses() {
    let hooks = fixture();
    let definition = hooks.hook(
        "check.sh",
        &format!(
            "head -c {} /dev/zero | tr '\\0' ' '\n{ALLOW}\n",
            ANSWER_BYTES_CAP
        ),
        None,
    );
    let refusal = refusal(hooks.run(&definition).await, RefusalCause::InvalidAnswer);
    assert!(refusal.reason.contains("ran past"), "{}", refusal.reason);
}

/// The timeout refuses on time and kills the script, so it does no more work
/// after its answer stopped mattering.
#[tokio::test]
async fn a_timeout_refuses_and_kills_the_script() {
    let hooks = fixture();
    let definition = hooks.hook("check.sh", "sleep 3\ntouch late\n", Some(1));
    let started = Instant::now();
    let refusal = refusal(hooks.run(&definition).await, RefusalCause::TimedOut);
    assert!(
        started.elapsed() < Duration::from_millis(2500),
        "{:?}",
        started.elapsed()
    );
    assert!(refusal.reason.contains("within 1s"), "{}", refusal.reason);

    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        !hooks.workspace.join("late").exists(),
        "the timed-out script ran on"
    );
}

/// The timeout kills a child the script started too, not just the script.
#[tokio::test]
async fn a_timeout_kills_what_the_script_started() {
    let hooks = fixture();
    let definition = hooks.hook("check.sh", "(sleep 3; touch late) &\nwait\n", Some(1));
    refusal(hooks.run(&definition).await, RefusalCause::TimedOut);

    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        !hooks.workspace.join("late").exists(),
        "the script's child ran on"
    );
}

/// A process the script left running holds its stdout open. The script's
/// answer still counts, and arrives when the script exits.
#[tokio::test]
async fn a_process_left_holding_stdout_does_not_hold_the_answer() {
    let hooks = fixture();
    let definition = hooks.hook("check.sh", &format!("sleep 5 &\n{ALLOW}\n"), Some(4));
    let started = Instant::now();
    let answer = hooks.run(&definition).await;
    assert_eq!(answer, HookAnswer::Allow { events: Vec::new() });
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}

/// A fresh workspace builds its Python venv on the first `.py` run. That
/// setup counts against the timeout, so a hook never holds its asker longer.
#[tokio::test]
async fn the_timeout_covers_python_setup() {
    let hooks = fixture();
    let definition = hooks.hook("check.py", "import time\ntime.sleep(3)\n", Some(1));
    let started = Instant::now();
    refusal(hooks.run(&definition).await, RefusalCause::TimedOut);
    // Venv creation alone takes over a second, so the bound sits close to
    // the timeout.
    assert!(
        started.elapsed() < Duration::from_millis(2000),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn the_timeout_stays_under_the_ceiling() {
    assert_eq!(
        effective_timeout(None),
        Duration::from_secs(DEFAULT_TIMEOUT_SECS)
    );
    assert_eq!(effective_timeout(Some(5)), Duration::from_secs(5));
    assert_eq!(effective_timeout(Some(0)), Duration::from_secs(1));
    assert_eq!(
        effective_timeout(Some(TIMEOUT_CEILING_SECS * 10)),
        Duration::from_secs(TIMEOUT_CEILING_SECS)
    );
}

#[tokio::test]
async fn a_missing_script_refuses() {
    let hooks = fixture();
    let definition = hooks.definition("gone.sh", None);
    let refusal = refusal(hooks.run(&definition).await, RefusalCause::Crashed);
    assert!(
        refusal.reason.contains("could not start"),
        "{}",
        refusal.reason
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_script_linked_out_of_its_folder_refuses() {
    let hooks = fixture();
    let outside = hooks.workspace.join("outside.sh");
    std::fs::write(&outside, ALLOW).unwrap();
    let definition = hooks.definition("check.sh", None);
    std::os::unix::fs::symlink(&outside, hooks.hook_dir().join("check.sh")).unwrap();
    let refusal = refusal(hooks.run(&definition).await, RefusalCause::Crashed);
    assert!(
        refusal.reason.contains("links outside"),
        "{}",
        refusal.reason
    );
}

#[tokio::test]
async fn a_hook_gets_the_environment_it_is_given() {
    let hooks = fixture();
    let definition = hooks.hook(
        "check.sh",
        &format!("printf '%s' \"$HOOK_PROBE\" > probe.txt\n{ALLOW}\n"),
        None,
    );
    let env = [("HOOK_PROBE".to_string(), "probe-value".to_string())];
    hooks
        .run_with(&definition, &input(), &Sandbox::unsandboxed(), &env)
        .await;
    assert_eq!(hooks.read("probe.txt"), "probe-value");
}

/// The script starts through the sandbox it is handed, never beside it.
#[tokio::test]
async fn a_hook_runs_in_its_sandbox() {
    let hooks = fixture();
    let definition = hooks.hook(
        "check.sh",
        &format!("printf '%s' \"${SANDBOXED_ENV}\" > sandboxed.txt\n{ALLOW}\n"),
        None,
    );
    hooks
        .run_with(&definition, &input(), &Sandbox::inherited(), &[])
        .await;
    assert_eq!(hooks.read("sandboxed.txt"), "1");
}

/// A secret the script holds, such as its database handle, never leaves in
/// its answer.
#[tokio::test]
async fn an_echoed_secret_is_masked() {
    let hooks = fixture();
    let url = "postgres://agent:pw-0123456789abcdef@localhost:5432/db";
    let definition = hooks.hook(
        "check.sh",
        &format!(
            "printf '{{\"answer\": \"refuse\", \"reason\": \"%s\"}}' \"${LUCIDOS_DATABASE_URL}\"\n"
        ),
        None,
    );
    let env = [(LUCIDOS_DATABASE_URL.to_string(), url.to_string())];
    let answer = hooks
        .run_with(&definition, &input(), &Sandbox::unsandboxed(), &env)
        .await;
    let refusal = refusal(answer, RefusalCause::Refused);
    assert!(
        !refusal.reason.contains("pw-0123456789abcdef"),
        "{}",
        refusal.reason
    );
}

/// The mask reaches every string a returned event carries, keys included.
#[tokio::test]
async fn a_secret_in_a_returned_event_is_masked() {
    let hooks = fixture();
    let url = "postgres://agent:pw-0123456789abcdef@localhost:5432/db";
    let definition = hooks.hook(
        "check.sh",
        &format!(
            r#"printf '{{"answer": "allow", "events": [{{"event_type": "TicketLinked", "payload": {{"%s": ["%s"]}}}}]}}' "${LUCIDOS_DATABASE_URL}" "${LUCIDOS_DATABASE_URL}""#
        ),
        None,
    );
    let env = [(LUCIDOS_DATABASE_URL.to_string(), url.to_string())];
    let answer = hooks
        .run_with(&definition, &input(), &Sandbox::unsandboxed(), &env)
        .await;
    let HookAnswer::Allow { events } = answer else {
        panic!("expected allow, got {answer:?}");
    };
    let payload = Value::Object(events[0].payload.clone()).to_string();
    assert!(!payload.contains("pw-0123456789abcdef"), "{payload}");
}

/// A masked event type would name some other event, so it is no answer.
#[tokio::test]
async fn a_secret_in_an_event_type_refuses() {
    let hooks = fixture();
    let url = "postgres://agent:pw-0123456789abcdef@localhost:5432/db";
    let definition = hooks.hook(
        "check.sh",
        &format!(
            r#"printf '{{"answer": "allow", "events": [{{"event_type": "%s"}}]}}' "${LUCIDOS_DATABASE_URL}""#
        ),
        None,
    );
    let env = [(LUCIDOS_DATABASE_URL.to_string(), url.to_string())];
    let answer = hooks
        .run_with(&definition, &input(), &Sandbox::unsandboxed(), &env)
        .await;
    let refusal = refusal(answer, RefusalCause::InvalidAnswer);
    assert!(
        refusal.reason.contains("holds a secret"),
        "{}",
        refusal.reason
    );
    assert!(
        !refusal.reason.contains("pw-0123456789abcdef"),
        "{}",
        refusal.reason
    );
}

#[tokio::test]
async fn a_python_hook_answers() {
    let hooks = fixture();
    let definition = hooks.hook(
        "check.py",
        "import json, sys\n\
         event = json.load(sys.stdin)\n\
         reason = event['hook'] + ' saw ' + event['event_type']\n\
         print(json.dumps({'answer': 'refuse', 'reason': reason}))\n",
        None,
    );
    let refusal = refusal(hooks.run(&definition).await, RefusalCause::Refused);
    assert_eq!(refusal.reason, "gate saw ChangeProposed");
}
