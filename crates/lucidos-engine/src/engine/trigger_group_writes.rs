//! Atomic create / rename helpers for trigger groups. They acquire
//! [`LucidosEngine::trigger_group_write_lock`] across read-dedup + emit +
//! projection apply, so the case-insensitive unique-name invariant survives
//! parallel HTTP POSTs and back-to-back LLM tool calls. See the field's
//! doc-comment for why a separate async mutex is needed.
//!
//! Exposed as free functions taking the registry / bus / lock trio so unit
//! tests can construct just those without spinning up the full engine. The
//! [`LucidosEngine`] methods are thin sugar over them.

use std::collections::HashMap;
use std::sync::Arc;

use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;
use crate::engine::LucidosEngine;
use crate::triggers::{find_group_by_name_ci, TriggerGroup};
use uuid::Uuid;

/// Successful outcome of [`create_trigger_group_locked`].
pub(crate) struct CreatedTriggerGroup {
    pub group_id: String,
    pub name: String,
    pub order: i32,
    pub created: chrono::DateTime<chrono::Utc>,
}

/// Failure modes for create. Callers translate these to their own response
/// shape (HTTP status code + JSON, or LLM-tool error string).
#[derive(Debug)]
pub(crate) enum CreateTriggerGroupError {
    /// Trimmed name was empty.
    EmptyName,
    /// A group whose name matches case-insensitively already exists. Carries
    /// the canonical existing name so the caller can echo it back in a 409
    /// instead of the requested-but-rejected casing.
    DuplicateName { existing_name: String },
    /// EventBus.emit returned an error; the string is the formatted source.
    EmitFailed(String),
}

/// Failure modes for rename.
#[derive(Debug)]
pub(crate) enum RenameTriggerGroupError {
    EmptyName,
    NotFound,
    DuplicateName { existing_name: String },
    EmitFailed(String),
}

/// Atomic create. Trims the name, holds `write_lock` across the dedup check,
/// the event emission, and the synchronous projection apply — so two
/// concurrent callers with the same name can't both observe an empty slot.
pub(crate) async fn create_trigger_group_locked(
    trigger_groups: &Arc<std::sync::RwLock<HashMap<String, TriggerGroup>>>,
    event_bus: &EventBus,
    write_lock: &tokio::sync::Mutex<()>,
    raw_name: &str,
    explicit_order: Option<i32>,
    actor: Option<MessageOrigin>,
) -> Result<CreatedTriggerGroup, CreateTriggerGroupError> {
    let name = raw_name.trim().to_string();
    if name.is_empty() {
        return Err(CreateTriggerGroupError::EmptyName);
    }

    let _guard = write_lock.lock().await;

    let order = {
        let g = trigger_groups.read().unwrap();
        if let Some(existing_id) = find_group_by_name_ci(&g, &name) {
            let existing_name = g
                .get(&existing_id)
                .expect("find_group_by_name_ci returned an id present in the same snapshot")
                .name
                .clone();
            return Err(CreateTriggerGroupError::DuplicateName { existing_name });
        }
        explicit_order.unwrap_or_else(|| g.values().map(|grp| grp.order).max().unwrap_or(0) + 10)
    };

    let group_id = Uuid::new_v4().to_string();
    let created = chrono::Utc::now();
    let payload = serde_json::json!({
        "group_id": group_id,
        "name": name,
        "order": order,
    });

    event_bus
        .emit(BusEvent::System(SystemEvent::TriggerGroupCreated {
            group_id: group_id.clone(),
            payload: payload.clone(),
            actor,
        }))
        .await
        .map_err(|e| CreateTriggerGroupError::EmitFailed(e.to_string()))?;

    crate::scheduler::handle_trigger_group_event(
        "TriggerGroupCreated",
        &group_id,
        &payload,
        trigger_groups,
    );

    Ok(CreatedTriggerGroup {
        group_id,
        name,
        order,
        created,
    })
}

/// Atomic rename. Same locking discipline as create; closes the
/// create-vs-rename and rename-vs-rename races where two writers could
/// converge on the same name.
pub(crate) async fn rename_trigger_group_locked(
    trigger_groups: &Arc<std::sync::RwLock<HashMap<String, TriggerGroup>>>,
    event_bus: &EventBus,
    write_lock: &tokio::sync::Mutex<()>,
    group_id: &str,
    raw_new_name: &str,
    actor: Option<MessageOrigin>,
) -> Result<(), RenameTriggerGroupError> {
    let new_name = raw_new_name.trim().to_string();
    if new_name.is_empty() {
        return Err(RenameTriggerGroupError::EmptyName);
    }

    let _guard = write_lock.lock().await;

    let existing_name = {
        let g = trigger_groups.read().unwrap();
        let existing = g.get(group_id).ok_or(RenameTriggerGroupError::NotFound)?;
        let existing_name = existing.name.clone();
        // A pure case-flip on the same id is a legitimate rename, not a
        // self-conflict, so dedup only fires when the names differ
        // case-insensitively.
        if !new_name.eq_ignore_ascii_case(&existing_name) {
            if let Some(conflict_id) = find_group_by_name_ci(&g, &new_name) {
                if conflict_id != group_id {
                    let conflict_name = g
                        .get(&conflict_id)
                        .expect("find_group_by_name_ci returned an id present in the same snapshot")
                        .name
                        .clone();
                    return Err(RenameTriggerGroupError::DuplicateName {
                        existing_name: conflict_name,
                    });
                }
            }
        }
        existing_name
    };

    if new_name == existing_name {
        return Ok(());
    }

    let payload = serde_json::json!({ "group_id": group_id, "name": new_name });
    event_bus
        .emit(BusEvent::System(SystemEvent::TriggerGroupRenamed {
            group_id: group_id.to_string(),
            payload: payload.clone(),
            actor,
        }))
        .await
        .map_err(|e| RenameTriggerGroupError::EmitFailed(e.to_string()))?;

    crate::scheduler::handle_trigger_group_event(
        "TriggerGroupRenamed",
        group_id,
        &payload,
        trigger_groups,
    );

    Ok(())
}

impl LucidosEngine {
    /// Sugar over [`create_trigger_group_locked`] that wires up the engine's
    /// own registry, bus, and write lock.
    pub(crate) async fn create_trigger_group_serialized(
        &self,
        raw_name: &str,
        explicit_order: Option<i32>,
        actor: Option<MessageOrigin>,
    ) -> Result<CreatedTriggerGroup, CreateTriggerGroupError> {
        create_trigger_group_locked(
            &self.trigger_groups,
            &self.event_bus,
            &self.trigger_group_write_lock,
            raw_name,
            explicit_order,
            actor,
        )
        .await
    }

    /// Sugar over [`rename_trigger_group_locked`].
    pub(crate) async fn rename_trigger_group_serialized(
        &self,
        group_id: &str,
        raw_new_name: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<(), RenameTriggerGroupError> {
        rename_trigger_group_locked(
            &self.trigger_groups,
            &self.event_bus,
            &self.trigger_group_write_lock,
            group_id,
            raw_new_name,
            actor,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    //! Concurrency tests against the locked helpers. The HTTP-level twin
    //! lives in `crates/lucidos-e2e/tests/api_support/trigger_groups_test.rs`.

    use super::*;
    use crate::test_support::setup_test_db;

    fn registry() -> Arc<std::sync::RwLock<HashMap<String, TriggerGroup>>> {
        Arc::new(std::sync::RwLock::new(HashMap::new()))
    }

    fn lock() -> tokio::sync::Mutex<()> {
        tokio::sync::Mutex::new(())
    }

    #[tokio::test]
    async fn concurrent_creates_with_same_name_yield_one_group() {
        let (pool, _db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let bus = Arc::new(bus);
        let groups = registry();
        let write_lock = Arc::new(lock());

        let name = "Race Group";
        let mut handles = Vec::new();
        for _ in 0..16 {
            let groups = groups.clone();
            let bus = bus.clone();
            let write_lock = write_lock.clone();
            let name = name.to_string();
            handles.push(tokio::spawn(async move {
                create_trigger_group_locked(&groups, &bus, &write_lock, &name, None, None).await
            }));
        }

        let mut ok = 0;
        let mut dup = 0;
        for h in handles {
            match h.await.expect("task panicked") {
                Ok(_) => ok += 1,
                Err(CreateTriggerGroupError::DuplicateName { .. }) => dup += 1,
                Err(CreateTriggerGroupError::EmptyName) => panic!("unexpected EmptyName"),
                Err(CreateTriggerGroupError::EmitFailed(m)) => {
                    panic!("unexpected EmitFailed: {}", m)
                }
            }
        }
        assert_eq!(ok, 1, "Exactly one create must succeed");
        assert_eq!(dup, 15, "Every other create must hit DuplicateName");

        let registry_snapshot = groups.read().unwrap();
        let matching: Vec<_> = registry_snapshot
            .values()
            .filter(|g| g.name == name)
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "Registry must hold exactly one row with name '{}' (got {})",
            name,
            matching.len()
        );
    }

    #[tokio::test]
    async fn concurrent_renames_to_same_target_block_one() {
        let (pool, _db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let bus = Arc::new(bus);
        let groups = registry();
        let write_lock = Arc::new(lock());

        let alpha = create_trigger_group_locked(&groups, &bus, &write_lock, "Alpha", None, None)
            .await
            .expect("create Alpha");
        let beta = create_trigger_group_locked(&groups, &bus, &write_lock, "Beta", None, None)
            .await
            .expect("create Beta");
        let gamma = create_trigger_group_locked(&groups, &bus, &write_lock, "Gamma", None, None)
            .await
            .expect("create Gamma");

        // Beta and Gamma both race to rename themselves to "Target". Exactly
        // one must succeed; the other must conflict. Alpha is the canary —
        // a buggy lock might let two writes interleave and rename Alpha by
        // mistake.
        let beta_id = beta.group_id.clone();
        let gamma_id = gamma.group_id.clone();
        let g1 = groups.clone();
        let b1 = bus.clone();
        let l1 = write_lock.clone();
        let g2 = groups.clone();
        let b2 = bus.clone();
        let l2 = write_lock.clone();
        let (r_beta, r_gamma) = tokio::join!(
            tokio::spawn(async move {
                rename_trigger_group_locked(&g1, &b1, &l1, &beta_id, "Target", None).await
            }),
            tokio::spawn(async move {
                rename_trigger_group_locked(&g2, &b2, &l2, &gamma_id, "Target", None).await
            }),
        );
        let beta_ok = r_beta.unwrap().is_ok();
        let gamma_ok = r_gamma.unwrap().is_ok();
        assert!(
            beta_ok ^ gamma_ok,
            "Exactly one rename must succeed (beta_ok={beta_ok} gamma_ok={gamma_ok})"
        );

        let registry_snapshot = groups.read().unwrap();
        // Alpha unchanged, exactly one "Target", loser still has its old name.
        assert!(
            registry_snapshot
                .values()
                .any(|g| g.id == alpha.group_id && g.name == "Alpha"),
            "Alpha must be untouched"
        );
        let target_count = registry_snapshot
            .values()
            .filter(|g| g.name == "Target")
            .count();
        assert_eq!(
            target_count, 1,
            "Exactly one group named 'Target' must exist"
        );
        let loser_name = if beta_ok { "Gamma" } else { "Beta" };
        assert!(
            registry_snapshot.values().any(|g| g.name == loser_name),
            "Loser ({}) must keep its original name",
            loser_name
        );
    }
}
