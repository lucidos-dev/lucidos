use crate::engine::agent_session::background_task::BackgroundTaskExt;
use crate::engine::tools::bash::BashExt;
use std::path::Path;

use uuid::Uuid;

use crate::engine::agent_session::background_task::AGENT_BG_DEFAULT_TIMEOUT_SECS;
use crate::engine::bash_background::{CompletionDelivery, TaskSpec};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::git_ops::{current_head_sha, git_cmd};
use crate::engine::permission::menu::{self, ALLOW_ONCE_LABEL, ALWAYS_ALLOW_LABEL};
use crate::engine::permission::{
    card_request, live_permission, raise_turn_holding_request, spend, tappable_request, Action,
    PermissionCardText, PermissionChoice, PermissionRow, PermissionUser, RequestCard,
    RequestOption,
};
use crate::engine::thread_events::{AnswerKind, EventChannel, EventMeta, ThreadEvent};
use crate::engine::LucidosEngine;
use crate::runtime::agent_sandbox::{Machine, Sandbox};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub(crate) const CONFINEMENT_COMMAND: &str = "./scripts/confinement-tests.sh --here";

fn confinement_command(tree: &TreeState) -> String {
    match tree.head() {
        Some(head) => format!("{CONFINEMENT_COMMAND} --revision {head}"),
        None => CONFINEMENT_COMMAND.to_string(),
    }
}

#[derive(Debug)]
pub(crate) enum ConfinementGate {
    Passed(CertifiedRun),
    Permitted(Box<PermissionRow>),
    Asked { card_id: String },
    Closed { card_id: String },
    Unpermitted,
    Inherited,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CertifiedRun {
    pub(crate) task_id: String,
    pub(crate) revision: String,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Asking<'a> {
    Owner,
    Nobody,
    Waiting { card_id: &'a str },
}

/// INVARIANT: One gate at a time, so two calls together raise one card and never spend
/// one once grant twice.
static GATING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn decide(
    bus: &EventBus,
    machine: &Machine,
    thread_id: Uuid,
    worktree: &Path,
    asking: Asking<'_>,
    _gating: &tokio::sync::MutexGuard<'static, ()>,
) -> Result<ConfinementGate, BoxError> {
    if *machine == Machine::Inherited {
        return Ok(ConfinementGate::Inherited);
    }
    if matches!(asking, Asking::Nobody) {
        let tree = tree_state(worktree).await;
        if let Some(run) = certified_run(bus, thread_id, tree.head()).await? {
            return Ok(ConfinementGate::Passed(run));
        }
    }
    let user = PermissionUser {
        thread_id: Some(thread_id),
        trigger_id: None,
    };
    let action = Action::RunConfinementTests;
    if let Some(permission) = spendable(bus, &user).await? {
        return Ok(ConfinementGate::Permitted(Box::new(permission)));
    }
    let tappable = tappable_request(bus.pool(), thread_id, action).await?;
    Ok(match (asking, tappable) {
        (Asking::Waiting { card_id }, Some(open)) if card_id == open => {
            ConfinementGate::Asked { card_id: open }
        }
        (Asking::Waiting { card_id }, _) => match granting_answer(bus, thread_id, card_id).await? {
            Some(GrantState::Landing) => ConfinementGate::Asked {
                card_id: card_id.to_string(),
            },
            Some(GrantState::Landed) => match spendable(bus, &user).await? {
                Some(permission) => ConfinementGate::Permitted(Box::new(permission)),
                None => ConfinementGate::Closed {
                    card_id: card_id.to_string(),
                },
            },
            None => ConfinementGate::Closed {
                card_id: card_id.to_string(),
            },
        },
        (Asking::Nobody, _) => ConfinementGate::Unpermitted,
        (Asking::Owner, Some(card_id)) => ConfinementGate::Asked { card_id },
        (Asking::Owner, None) => ConfinementGate::Asked {
            card_id: raise_turn_holding_request(
                bus,
                thread_id,
                confinement_card(thread_id),
                EventChannel::ClaudeCode,
            )
            .await?,
        },
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TreeState {
    Clean { head: String },
    Dirty,
    Unknown,
}

impl TreeState {
    pub(crate) fn head(&self) -> Option<&str> {
        match self {
            TreeState::Clean { head } => Some(head),
            TreeState::Dirty | TreeState::Unknown => None,
        }
    }
}

/// INVARIANT: The worktree's state, read with flags that override the user's git
/// config, so a setting cannot hide an untracked file or a submodule edit.
async fn tree_state(worktree: &Path) -> TreeState {
    let status = git_cmd(
        &[
            "-c",
            "core.fsmonitor=false",
            "status",
            "--porcelain",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
        worktree,
    )
    .await;
    let listing = git_cmd(&["ls-files", "-v", "-z"], worktree).await;
    let (Ok(status), Ok(listing)) = (status, listing) else {
        return TreeState::Unknown;
    };
    if !status.status.success() || !listing.status.success() {
        return TreeState::Unknown;
    }
    if !status.stdout.is_empty() || hides_paths(&listing.stdout) {
        return TreeState::Dirty;
    }
    match current_head_sha(worktree).await {
        Some(head) if !head.is_empty() && head.bytes().all(|b| b.is_ascii_hexdigit()) => {
            TreeState::Clean { head }
        }
        _ => TreeState::Unknown,
    }
}

fn hides_paths(listing: &[u8]) -> bool {
    listing
        .split(|byte| *byte == 0)
        .filter_map(|entry| entry.first())
        .any(|tag| tag.is_ascii_lowercase() || *tag == b'S')
}

#[derive(serde::Deserialize)]
struct TaskEnd {
    exit_code: Option<i32>,
    #[serde(default)]
    timed_out: bool,
    #[serde(default)]
    killed: bool,
    #[serde(default)]
    abandoned: bool,
}

impl TaskEnd {
    fn passed(&self) -> bool {
        self.exit_code == Some(0) && !(self.timed_out || self.killed || self.abandoned)
    }
}

async fn certified_run(
    bus: &EventBus,
    thread_id: Uuid,
    clean_head: Option<&str>,
) -> Result<Option<CertifiedRun>, BoxError> {
    let Some(head) = clean_head else {
        return Ok(None);
    };
    let latest: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT payload->>'task_id', payload->>'revision' FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ConfinementRunStarted' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id.to_string())
    .fetch_optional(bus.pool())
    .await?;
    let Some((task_id, Some(revision))) = latest else {
        return Ok(None);
    };
    if revision != head {
        return Ok(None);
    }
    let end: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload FROM events \
         WHERE aggregate_id = $1 AND event_type = 'BackgroundBashCompleted' \
           AND payload->>'task_id' = $2 \
         ORDER BY sequence LIMIT 1",
    )
    .bind(thread_id.to_string())
    .bind(&task_id)
    .fetch_optional(bus.pool())
    .await?;
    let passed = match end.map(serde_json::from_value::<TaskEnd>).transpose()? {
        Some(end) => end.passed(),
        None => false,
    };
    Ok(passed.then_some(CertifiedRun { task_id, revision }))
}

async fn spendable(
    bus: &EventBus,
    user: &PermissionUser,
) -> Result<Option<PermissionRow>, BoxError> {
    let action = Action::RunConfinementTests;
    Ok(
        match live_permission(bus.pool(), user, action, None).await? {
            Some(permission) if spend(bus, &permission).await => Some(permission),
            _ => None,
        },
    )
}

#[derive(Debug, PartialEq, Eq)]
enum GrantState {
    Landing,
    Landed,
}

async fn granting_answer(
    bus: &EventBus,
    thread_id: Uuid,
    card_id: &str,
) -> Result<Option<GrantState>, BoxError> {
    let answer: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload->'answer' FROM events \
         WHERE aggregate_id = $1 AND event_type = 'UserQuestionAnswered' \
           AND payload->>'tool_use_id' = $2 \
         ORDER BY sequence LIMIT 1",
    )
    .bind(thread_id.to_string())
    .bind(card_id)
    .fetch_optional(bus.pool())
    .await?;
    let Some(AnswerKind::Selected { option_id }) =
        answer.map(serde_json::from_value).transpose()?
    else {
        return Ok(None);
    };
    let Some(request) = card_request(bus.pool(), thread_id, card_id).await? else {
        return Ok(None);
    };
    if !matches!(
        request.choices.get(&option_id),
        Some(PermissionChoice::Grant { .. })
    ) {
        return Ok(None);
    }
    let landed: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM permissions WHERE request_tool_use_id = $1)",
    )
    .bind(card_id)
    .fetch_one(bus.pool())
    .await?;
    Ok(Some(if landed {
        GrantState::Landed
    } else {
        GrantState::Landing
    }))
}

pub(crate) fn confinement_card(thread_id: Uuid) -> RequestCard {
    RequestCard {
        question: PermissionCardText::new("Run the confinement tests outside the sandbox?")
            .paragraph(
                "They prove what the agent sandbox denies, and this machine cannot nest one \
                 sandbox inside another. The run executes this branch's code with no sandbox, \
                 so it can read Lucidos's secrets and every file your account can.",
            )
            .finish(None),
        header: "Sandbox",
        action: Action::RunConfinementTests,
        breadth: None,
        options: menu::options(
            Action::RunConfinementTests,
            thread_id,
            "Nothing runs. Hardening reports the confinement suite as not run.",
        )
        .into_iter()
        .map(|option| RequestOption {
            description: match option.label.as_str() {
                ALLOW_ONCE_LABEL => {
                    "Run them this once, for this thread. The next run asks again.".to_string()
                }
                ALWAYS_ALLOW_LABEL => "Every future confinement run in this workspace runs \
                                       with no sandbox and no card, from any thread, until \
                                       you revoke it in Settings → Permissions."
                    .to_string(),
                _ => option.description,
            },
            ..option
        })
        .collect(),
        batch: None,
    }
}

#[derive(Debug)]
pub(crate) enum ConfinementRun {
    /// INVARIANT: Running as a polled background task.
    Started {
        task_id: String,
        timeout_secs: u64,
        tree: TreeState,
    },
    Passed(CertifiedRun),
    Asked {
        card_id: String,
    },
    Closed {
        card_id: String,
    },
    Unpermitted,
    Refused {
        reason: String,
    },
}

#[easy_ext::ext(ConfinementRunExt)]
pub(crate) impl LucidosEngine {
    async fn run_confinement_tests_for_agent(
        &self,
        thread_id: Uuid,
        asking: Asking<'_>,
    ) -> Result<ConfinementRun, String> {
        let Ok(cwd) = self.agent_worktree(thread_id).await else {
            return Ok(ConfinementRun::Refused {
                reason: NO_WORKTREE_REFUSAL.to_string(),
            });
        };
        let machine = crate::runtime::agent_sandbox::machine().await;
        // INVARIANT: Held until the run is on record, so no call reads the evidence
        // while a run it should see is still starting.
        let _gating = GATING.lock().await;
        let permission = match decide(&self.event_bus, &machine, thread_id, &cwd, asking, &_gating)
            .await
            .map_err(|e| format!("The confinement run could not check its permission: {e}"))?
        {
            ConfinementGate::Passed(run) => return Ok(ConfinementRun::Passed(run)),
            ConfinementGate::Permitted(permission) => *permission,
            ConfinementGate::Asked { card_id } => return Ok(ConfinementRun::Asked { card_id }),
            ConfinementGate::Closed { card_id } => return Ok(ConfinementRun::Closed { card_id }),
            ConfinementGate::Unpermitted => return Ok(ConfinementRun::Unpermitted),
            ConfinementGate::Inherited => {
                return Ok(ConfinementRun::Refused {
                    reason: INHERITED_REFUSAL.to_string(),
                })
            }
        };
        let sandbox = Sandbox::outside_for_confinement(&permission)
            .ok_or("The permission the gate found does not cover a confinement run.")?;
        let env_vars = self
            .build_agent_task_env_vars(thread_id, &cwd, sandbox.compile_cache())
            .await?;
        let tree = tree_state(&cwd).await;
        let spec = TaskSpec {
            timeout_secs: AGENT_BG_DEFAULT_TIMEOUT_SECS,
            description: Some("the confinement tests, outside the sandbox"),
            delivery: CompletionDelivery::Poll,
            inherited: crate::core::InheritedSecrets::Keep,
        };
        let (task_id, _started_at) = self
            .start_background_task(
                &sandbox,
                thread_id,
                &confinement_command(&tree),
                &cwd,
                &env_vars,
                spec,
            )
            .await?;
        let started = ThreadEvent::ConfinementRunStarted {
            task_id: task_id.clone(),
            permission_id: permission.permission_id,
            revision: tree.head().map(str::to_string),
        };
        // INVARIANT: A run off the record could fail unseen while an older pass at the
        // same commit still certifies it, so it does not go on.
        if let Err(e) = self
            .event_bus
            .emit(BusEvent::Thread {
                thread_id,
                event: started,
                meta: EventMeta::NONE,
            })
            .await
        {
            self.stop_background_task_as(thread_id, &task_id).await;
            return Err(format!(
                "The confinement run could not be recorded, so it was stopped: {e}"
            ));
        }
        Ok(ConfinementRun::Started {
            task_id,
            timeout_secs: AGENT_BG_DEFAULT_TIMEOUT_SECS,
            tree,
        })
    }
}

pub(crate) const NO_WORKTREE_REFUSAL: &str =
    "This thread has no live coding-agent session with a worktree, so the confinement tests \
     have nowhere to run. Run them from the coding-agent thread whose branch they test.";

pub(crate) const INHERITED_REFUSAL: &str =
    "This engine runs inside another Lucidos sandbox, so nothing it starts can leave that \
     sandbox. Run the confinement tests from an engine that runs bare.";

#[cfg(test)]
#[path = "confinement_run_tests.rs"]
mod tests;
