//! Turns a provider's not-found answer into what the user sees: a
//! `ModelNotServedObserved` event, which also moves later defaults, and a
//! notification when a stored pick failed for good (ADR 0403).

use async_trait::async_trait;
use sqlx::PgPool;

use super::reach::{Reach, NOT_SERVED_EVENT, NOT_SERVED_WINDOW};
use super::ModelSource;
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::ContextPurpose;
use crate::llm::model_registry::ModelRegistry;
use crate::llm::{ModelNotServed, NotServedReport};
use crate::scheduler::notifications::{NavigateTarget, NavigateUi, Tap};

/// Serializes the check and the emit, so two calls failing at once record one
/// event and one notification. One engine serves a workspace, so a process
/// lock is enough, and only a failure takes it.
static REPORTING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Reports one auxiliary call's not-found answers.
pub(crate) struct NotServedRecorder {
    pub(crate) bus: EventBus,
    pub(crate) pool: PgPool,
    pub(crate) registry: ModelRegistry,
    pub(crate) reach: Reach,
    pub(crate) purpose: ContextPurpose,
    pub(crate) source: ModelSource,
}

impl NotServedRecorder {
    /// Whether this provider, model and purpose already have a row inside the
    /// window, with the same outcome. A call that moved on and one that failed
    /// count apart. So a failed stored pick still notifies after a default
    /// moved past the same model.
    ///
    /// A failed read counts as yes: a missed event costs one call's
    /// visibility, a duplicate costs the user a second notification.
    async fn observed_already(&self, provider: &str, model: &str, failed: bool) -> bool {
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM events WHERE event_type = $1 \
             AND created >= now() - make_interval(secs => $2) \
             AND payload->'data'->>'provider' = $3 AND payload->'data'->>'model' = $4 \
             AND payload->'data'->>'purpose' = $5 \
             AND (payload->'data'->'moved_to' IS NULL) = $6)",
        )
        .bind(NOT_SERVED_EVENT)
        .bind(NOT_SERVED_WINDOW.as_secs() as f64)
        .bind(provider)
        .bind(model)
        .bind(purpose_wire(self.purpose))
        .bind(failed)
        .fetch_one(&self.pool)
        .await
        .unwrap_or_else(|e| {
            crate::log!("[AuxPurpose] could not check not-served events: {}", e);
            true
        })
    }

    async fn notify(&self, provider: &str, model: &str, refusal: &ModelNotServed) {
        let task = task_name(self.purpose);
        self.bus
            .emit_or_log(
                BusEvent::System(SystemEvent::NotificationCreated {
                    id: uuid::Uuid::new_v4().to_string(),
                    title: format!("{model} is no longer served"),
                    message: format!(
                        "{provider} answered that it does not serve {model}, the model \
                         picked for {task}, so {task} did not run. Pick another model \
                         under Settings, Models, Background tasks.\n\n{refusal}"
                    ),
                    task_id: None,
                    app_id: None,
                    thread_id: None,
                    event_id: None,
                    tap: Tap::Navigate {
                        to: Box::new(NavigateUi {
                            target: NavigateTarget::Settings,
                            settings_view: Some("models".to_string()),
                            ..Default::default()
                        }),
                    },
                    actor: None,
                }),
                "[AuxPurpose] not-served NotificationCreated",
            )
            .await;
    }
}

#[async_trait]
impl NotServedReport for NotServedRecorder {
    async fn not_served(&self, model: &str, refusal: &ModelNotServed, moved_to: Option<&str>) {
        let Some(route) = self.reach.route(&self.registry, model) else {
            return;
        };
        let provider = route.provider.as_str();
        crate::log!(
            "[AuxPurpose] {} does not serve {} for {:?}; {}",
            provider,
            model,
            self.purpose,
            moved_to.map_or("the call failed".to_string(), |m| format!("moved to {m}"))
        );
        let _serialized = REPORTING.lock().await;
        if self
            .observed_already(provider, model, moved_to.is_none())
            .await
        {
            return;
        }
        self.bus
            .emit_or_log(
                BusEvent::System(SystemEvent::ModelNotServedObserved {
                    model: model.to_string(),
                    provider: provider.to_string(),
                    purpose: self.purpose,
                    message: refusal.to_string(),
                    moved_to: moved_to.map(str::to_string),
                }),
                "[AuxPurpose] ModelNotServedObserved",
            )
            .await;
        if moved_to.is_none() && self.source == ModelSource::Preference {
            self.notify(provider, model, refusal).await;
        }
    }
}

/// How the event payload spells `purpose`.
fn purpose_wire(purpose: ContextPurpose) -> String {
    serde_json::to_value(purpose)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// The task `purpose` names in prose: its capture's section name, lower-cased
/// and without the "Request" suffix ("Command Judge Request" reads "command
/// judge").
fn task_name(purpose: ContextPurpose) -> String {
    let section = purpose.section_name();
    section
        .strip_suffix(" Request")
        .unwrap_or(section)
        .to_lowercase()
}

#[cfg(test)]
#[path = "not_served_report_tests.rs"]
mod tests;
