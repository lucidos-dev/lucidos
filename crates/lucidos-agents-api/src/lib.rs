//! Agent execution and the HTTP API.
#![deny(clippy::print_stdout, clippy::print_stderr)]

#[macro_use]
extern crate lucidos_engine;

pub use lucidos_engine::*;
pub mod api;
pub mod engine;

#[cfg(test)]
pub mod test_support {
    pub use lucidos_engine::test_support::*;

    pub async fn setup_test_db() -> (sqlx::PgPool, String) {
        crate::core::secret_store::key_store::install_test_key_store();
        lucidos_engine::test_support::setup_test_db().await
    }

    pub async fn boot_mock_engine(
        db: &str,
    ) -> (
        std::sync::Arc<crate::engine::LucidosEngine>,
        tempfile::TempDir,
    ) {
        boot_mock_engine_with_host(
            db,
            std::sync::Arc::new(crate::engine::agent_recovery::EngineAgentHost),
        )
        .await
    }
}

#[cfg(test)]
mod guard_tests;

#[cfg(test)]
pub mod core;

#[cfg(test)]
#[path = "llm/test_facade.rs"]
pub mod llm;

#[cfg(test)]
pub mod runtime;
