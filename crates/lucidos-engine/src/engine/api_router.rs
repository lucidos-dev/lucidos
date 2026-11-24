//! The engine's handle on its own `/api/v1` router, so in-process callers run
//! a request through every layer a live one passes.
//!
//! Two callers: the owner door replays an act the owner allowed once
//! (`api::owner_door`), and a route-backed tool sends its CLI's request
//! (`engine::tools::route_dispatch`, ADR 0018).

use super::LucidosEngine;

impl LucidosEngine {
    /// Register the built router. `api::create_router` calls it once. A second
    /// call, as from a second router built in a test, keeps the first.
    pub fn set_api_router(&self, router: axum::Router) {
        if self.api_router.set(router).is_err() {
            crate::log_legacy!("[ApiRouter] The router was already set; keeping the first");
        }
    }

    /// The registered router, or `None` before `api::create_router` ran.
    pub fn api_router(&self) -> Option<axum::Router> {
        self.api_router.get().cloned()
    }
}
