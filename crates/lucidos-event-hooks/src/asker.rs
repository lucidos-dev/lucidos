use std::borrow::Cow;
use std::path::Path;
use std::sync::{Arc, Weak};

use lucidos_core::core::agent_sandbox::Sandbox;
use lucidos_core::core::event_hooks::{
    effective_timeout, ArmedHook, EventHookAsker, HookAsk, HookChoice, HookDefinition, HookListing,
    HookVerdict, PendingEvent, Refusal, RefusalCause, HOOK_CHOICE_LIMIT, HOOK_CHOICE_QUERY_LIMIT,
};
use lucidos_core::core::event_subscription::EventSubscription;
use lucidos_engine::engine::LucidosEngine;
use serde_json::Value;
use tokio::sync::watch;

use crate::staged::{clear_stale, StagedHook};
use crate::{run_hook, Confinement, HookAnswer, HookInput};

pub struct WorkspaceHookAsker {
    engine: Weak<LucidosEngine>,
    serving: watch::Receiver<bool>,
    choice_slots: tokio::sync::Semaphore,
}

impl WorkspaceHookAsker {
    pub fn new(engine: &Arc<LucidosEngine>, serving: watch::Receiver<bool>) -> Self {
        clear_stale(engine.workspace_path());
        Self {
            engine: Arc::downgrade(engine),
            serving,
            choice_slots: tokio::sync::Semaphore::new(4),
        }
    }
}

async fn change_context(engine: &LucidosEngine, pending: &PendingEvent) -> Option<Value> {
    let is_apply = match pending.event_type.as_str() {
        "ChangeProposed" => false,
        "ChangeApplyRequested" => true,
        _ => return None,
    };
    let repo_root = Path::new(pending.payload["repo_root"].as_str()?);
    let branch_name = pending.payload["branch_name"].as_str()?;
    let thread_id = pending.payload["thread_id"].as_str()?.parse().ok()?;
    let kind =
        lucidos_engine::engine::change_ops::load_apply_kind_context(engine.pool(), Some(thread_id))
            .await
            .kind;
    let marker = engine.plan_marker_snapshot(repo_root, branch_name).await;
    let landing_files = if is_apply {
        engine.landing_files(repo_root, branch_name).await
    } else {
        serde_json::from_value(pending.payload["files"].clone()).ok()
    };
    Some(lucidos_engine::engine::change_ops::change_plan_context(
        kind,
        &marker,
        landing_files.as_deref(),
    ))
}

#[async_trait::async_trait]
impl EventHookAsker for WorkspaceHookAsker {
    async fn ask(&self, ask: HookAsk<'_>) -> HookVerdict {
        let slug = ask.hook.slug.as_str();
        let unavailable = |why: String| {
            HookVerdict::Refused(Refusal {
                cause: RefusalCause::Unavailable,
                reason: format!("The hook '{slug}' could not be asked: {why}"),
                detail: None,
            })
        };
        let Some(engine) = self.engine.upgrade() else {
            return unavailable("the engine is stopping.".to_string());
        };
        let workspace = engine.workspace_path();
        let (staged, definition) = match checked_copy(workspace, ask.hook) {
            Ok(checked) => checked,
            Err(refusal) => return HookVerdict::Refused(refusal),
        };
        if !condition_matches(&definition, ask.pending) {
            return HookVerdict::Skipped;
        }
        if self
            .serving
            .clone()
            .wait_for(|serving| *serving)
            .await
            .is_err()
        {
            return unavailable("the engine stopped before it served.".to_string());
        }
        let arguments = crate::input::declared_args(
            ask.arguments,
            definition.arguments.keys().map(String::as_str),
        );
        let creations = crate::input::declared_creations(
            ask.arguments,
            slug,
            definition
                .arguments
                .iter()
                .map(|(name, argument)| (name.as_str(), argument)),
        );
        let mut pending = Cow::Borrowed(ask.pending);
        if let Some(context) = change_context(&engine, &pending).await {
            pending.to_mut().payload["change_context"] = context;
        }
        let input = match HookInput::read(engine.pool(), &pending, &arguments, &creations).await {
            Ok(input) => input,
            Err(e) => return unavailable(e),
        };
        let sandbox = match Sandbox::resolve(engine.pool(), workspace).await {
            Ok(sandbox) => sandbox,
            Err(e) => return unavailable(format!("its sandbox could not be set up: {e}")),
        };
        let env = engine
            .build_event_hook_env_vars(ask.chain_depth, slug)
            .await;
        let confinement = Confinement {
            sandbox: &sandbox,
            python: &engine.python_runtime,
            env: &env,
        };
        let hook_dir = staged.root().join(slug);
        let started = tokio::time::Instant::now();
        let mut verdict = verdict_of(
            &definition,
            run_hook(
                workspace,
                &hook_dir,
                slug,
                &definition,
                &input,
                &confinement,
            )
            .await,
        );
        if let HookVerdict::NeedsInput {
            argument, choices, ..
        } = &mut verdict
        {
            if ask.can_answer {
                let remaining =
                    effective_timeout(definition.timeout_secs).saturating_sub(started.elapsed());
                if let Ok(Ok(top)) =
                    tokio::time::timeout(remaining, self.choices(ask.hook, argument, "")).await
                {
                    choices.extend(top.into_iter().take(8));
                }
            }
        }
        verdict
    }

    async fn choices(
        &self,
        hook: &ArmedHook,
        name: &str,
        query: &str,
    ) -> Result<Vec<HookChoice>, String> {
        if query.len() > HOOK_CHOICE_QUERY_LIMIT {
            return Err("The choices query is too long.".into());
        }
        let _slot = self
            .choice_slots
            .try_acquire()
            .map_err(|_| "Hook choices are busy. Try again.")?;
        let engine = self.engine.upgrade().ok_or("The engine is stopping.")?;
        let workspace = engine.workspace_path();
        let (staged, definition) = checked_copy(workspace, hook).map_err(|e| e.reason)?;
        let argument = definition
            .arguments
            .get(name)
            .ok_or("This argument is not declared.")?;
        let sandbox = Sandbox::resolve(engine.pool(), workspace)
            .await
            .map_err(|e| e.to_string())?
            .for_hook_choices()
            .await
            .ok_or("Hook choices need a sandbox that confines file contents and metadata.")?;
        let env = engine.build_event_hook_choices_env_vars().await;
        let mut secrets = lucidos_engine::engine::engine_impl::stored_secret_values(engine.pool())
            .await
            .map_err(|_| "Could not prepare the choices query.")?;
        secrets.extend(lucidos_core::core::subprocess_secret_values(
            &env,
            lucidos_core::core::inherited_secret_env(),
        ));
        let input = serde_json::json!({
            "hook": hook.slug,
            "argument": name,
            "query": query,
            "limit": HOOK_CHOICE_LIMIT,
        });
        crate::runner::run_choices(
            workspace,
            &staged.root().join(&hook.slug),
            &argument.choices_script,
            definition.timeout_secs,
            input,
            &Confinement {
                sandbox: &sandbox,
                python: &engine.python_runtime,
                env: &env,
            },
            &secrets,
        )
        .await
    }
}

fn checked_copy(
    workspace: &Path,
    armed: &ArmedHook,
) -> Result<(StagedHook, HookDefinition), Refusal> {
    let staged = StagedHook::stage(workspace, &armed.slug)?;
    let changed = |why: String| Refusal {
        cause: RefusalCause::Changed,
        reason: format!(
            "The hook '{}' {why} The owner arms it again or disarms it.",
            armed.slug
        ),
        detail: None,
    };
    match crate::load(staged.root(), armed.slug.clone()) {
        HookListing::Valid {
            definition,
            content_hash,
            ..
        } if content_hash == armed.content_hash => Ok((staged, definition)),
        HookListing::Valid { .. } => Err(changed("changed since the owner armed it.".to_string())),
        HookListing::Invalid { problems, .. } => Err(changed(format!(
            "is no longer valid: {}",
            problems.join(" ")
        ))),
    }
}

fn condition_matches(definition: &HookDefinition, pending: &PendingEvent) -> bool {
    EventSubscription {
        event_type: definition.event_type.clone(),
        condition: definition.condition.clone(),
    }
    .matches(&pending.event_type, &pending.payload)
}

fn verdict_of(definition: &HookDefinition, answer: HookAnswer) -> HookVerdict {
    match answer {
        HookAnswer::Allow { events } => HookVerdict::Allowed(events),
        HookAnswer::NeedsInput { argument } => match definition.arguments.get(&argument) {
            Some(declared) => HookVerdict::NeedsInput {
                argument,
                label: declared.label.clone(),
                create_label: declared.create_label.clone(),
                choices_timeout_secs: effective_timeout(definition.timeout_secs).as_secs(),
                choices: Vec::new(),
            },
            None => HookVerdict::Refused(Refusal {
                cause: RefusalCause::InvalidAnswer,
                reason: format!("The hook asked for the undeclared argument '{argument}'."),
                detail: None,
            }),
        },
        HookAnswer::Refuse(refusal) => HookVerdict::Refused(refusal),
    }
}

#[cfg(test)]
#[path = "asker_tests.rs"]
mod tests;
