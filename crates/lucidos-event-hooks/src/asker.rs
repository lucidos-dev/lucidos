use std::path::Path;
use std::sync::{Arc, Weak};

use lucidos_core::core::agent_sandbox::Sandbox;
use lucidos_core::core::event_hooks::{
    ArmedHook, EventHookAsker, HookAsk, HookDefinition, HookListing, HookVerdict, PendingEvent,
    Refusal, RefusalCause,
};
use lucidos_core::core::event_subscription::EventSubscription;
use lucidos_engine::engine::LucidosEngine;
use tokio::sync::watch;

use crate::staged::{clear_stale, StagedHook};
use crate::{run_hook, Confinement, HookAnswer, HookInput};

pub struct WorkspaceHookAsker {
    engine: Weak<LucidosEngine>,
    serving: watch::Receiver<bool>,
}

impl WorkspaceHookAsker {
    pub fn new(engine: &Arc<LucidosEngine>, serving: watch::Receiver<bool>) -> Self {
        clear_stale(engine.workspace_path());
        Self {
            engine: Arc::downgrade(engine),
            serving,
        }
    }
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
        let input = match HookInput::read(engine.pool(), ask.pending).await {
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
        verdict_of(
            slug,
            run_hook(
                workspace,
                &hook_dir,
                slug,
                &definition,
                &input,
                &confinement,
            )
            .await,
        )
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

fn verdict_of(slug: &str, answer: HookAnswer) -> HookVerdict {
    match answer {
        HookAnswer::Allow { events } => HookVerdict::Allowed(events),
        HookAnswer::NeedsInput { argument } => HookVerdict::Refused(Refusal {
            cause: RefusalCause::MissingInput,
            reason: format!(
                "The hook '{slug}' needs the argument '{argument}', and none was given."
            ),
            detail: None,
        }),
        HookAnswer::Refuse(refusal) => HookVerdict::Refused(refusal),
    }
}

#[cfg(test)]
#[path = "asker_tests.rs"]
mod tests;
