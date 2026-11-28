#![deny(clippy::print_stdout, clippy::print_stderr)]

#[macro_use]
extern crate lucidos_engine;

pub use lucidos_engine::*;
pub mod api;
pub mod engine;

#[cfg(feature = "test-support")]
#[path = "core/snapshot/recovered_compose_http_tests.rs"]
mod recovered_compose_http_tests;

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod test_support {
    #[cfg(feature = "test-support")]
    pub use crate::api::permission_requests::test_fixtures::{
        assert_snapshot_grant_expired, seed_snapshot_grant,
    };
    #[cfg(feature = "test-support")]
    pub use crate::engine::chat::recovery::recover_orphaned_threads;
    #[cfg(feature = "test-support")]
    pub use crate::recovered_compose_http_tests::verify_recovered_compose;
    #[cfg(test)]
    pub use lucidos_engine::test_support::*;

    #[cfg(test)]
    pub async fn setup_test_db() -> (sqlx::PgPool, String) {
        crate::core::secret_store::key_store::install_test_key_store();
        lucidos_engine::test_support::setup_test_db().await
    }

    #[cfg(test)]
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
mod engine_version_facts_tests;

#[cfg(test)]
mod guard_tests;

#[cfg(test)]
pub mod core;

#[cfg(test)]
#[path = "llm/test_facade.rs"]
pub mod llm;

#[cfg(test)]
pub mod runtime;
