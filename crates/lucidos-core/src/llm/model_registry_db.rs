use std::collections::HashMap;

use sqlx::PgPool;

use super::model_registry::{routes_from_models, ModelRegistry, ModelRouting};
use crate::core::ModelStore;

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
