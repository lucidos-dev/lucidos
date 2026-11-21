//! Test helpers that construct the full engine.

use crate::test_support::test_db_url;

/// A whole engine on database `db`, with the mock model. Keep the workspace
/// alive for as long as the engine runs.
#[cfg(test)]
pub async fn boot_mock_engine(
    db: &str,
) -> (
    std::sync::Arc<crate::engine::LucidosEngine>,
    tempfile::TempDir,
) {
    boot_mock_engine_with_host(
        db,
        std::sync::Arc::new(crate::engine::agent_host::NoAgentHost::default()),
    )
    .await
}

/// A mock-model engine with the caller's session host and an isolated workspace.
pub async fn boot_mock_engine_with_host(
    db: &str,
    agent_host: std::sync::Arc<dyn crate::engine::agent_host::AgentHost>,
) -> (
    std::sync::Arc<crate::engine::LucidosEngine>,
    tempfile::TempDir,
) {
    let workspace = tempfile::tempdir().unwrap();
    let engine = std::sync::Arc::new(
        crate::engine::LucidosEngine::new(
            workspace.path().to_path_buf(),
            &test_db_url(db),
            crate::llm::ProviderBuildContext {
                default_model: "mock".into(),
                model_is_mock: true,
                vertex_project_id: String::new(),
                vertex_location: crate::llm::vertex::location_handle(String::new()),
                vertex_token_cache: None,
                model_registry: Default::default(),
                not_served: Default::default(),
                claude_gateway_access: None,
                chatgpt_plan_access: None,
            },
            agent_host,
        )
        .await
        .unwrap(),
    );
    engine.set_self_arc(&engine);
    (engine, workspace)
}

/// Returns the same 384-dim vector for every text. pgvector stores
/// `vector(384)`, so anything that writes a memory entry needs this width,
/// and every pair of texts is maximally similar.
pub struct Fixed384Embedder;

#[async_trait::async_trait]
impl crate::memory::EmbeddingProvider for Fixed384Embedder {
    async fn embed(
        &self,
        _text: &str,
    ) -> Result<Vec<f32>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(vec![0.1f32; 384])
    }
    async fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(texts.iter().map(|_| vec![0.1f32; 384]).collect())
    }
    fn dimensions(&self) -> usize {
        384
    }
    fn model_id(&self) -> &str {
        "test-fixed-384"
    }
}
