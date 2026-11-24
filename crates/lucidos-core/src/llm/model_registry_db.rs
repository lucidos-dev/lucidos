//! Load the model registry and replace the shared routing map.

use std::collections::HashMap;

use sqlx::PgPool;

use super::model_registry::{routes_from_models, ModelRegistry, ModelRouting};
use crate::core::ModelStore;

/// Load all rows from the `models` table. A saved `chat_model` can still name
/// a model that was later disabled. On a DB error, return an empty map and log; routing
/// then degrades to the prefix heuristic rather than failing every call.
pub async fn load_from_db(pool: &PgPool) -> HashMap<String, ModelRouting> {
    match ModelStore::list(pool).await {
        Ok(models) => routes_from_models(models),
        Err(e) => {
            crate::log_legacy!(
                "[ModelRegistry] Failed to load models table; routing falls back to prefix heuristic: {}",
                e
            );
            HashMap::new()
        }
    }
}

/// Refill `registry` from the `models` table, replacing what it held.
pub async fn reload(registry: &ModelRegistry, pool: &PgPool) {
    let fresh = load_from_db(pool).await;
    match registry.write() {
        Ok(mut guard) => {
            *guard = fresh;
            crate::log_legacy!(
                "[ModelRegistry] reloaded model→provider map ({} entries)",
                guard.len()
            );
        }
        Err(e) => crate::log_legacy!("[ModelRegistry] reload skipped (lock poisoned): {}", e),
    }
}
