use super::LucidosEngine;

impl LucidosEngine {
    pub fn set_api_router(&self, router: axum::Router) {
        if self.api_router.set(router).is_err() {
            crate::log_legacy!("[ApiRouter] The router was already set; keeping the first");
        }
    }

    pub fn api_router(&self) -> Option<axum::Router> {
        self.api_router.get().cloned()
    }
}
