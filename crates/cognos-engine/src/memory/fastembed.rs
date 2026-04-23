use async_trait::async_trait;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use std::sync::Arc;

use super::provider::EmbeddingProvider;

// These string values are also referenced by SQL migrations
// (e.g. memory_entries_embedding_model.sql backfills 'bge-small-en-v1.5').
// Renaming the const is fine; changing the string value will desync the migration backfill.
pub const MODEL_BGE_SMALL_EN: &str = "bge-small-en-v1.5";
pub const MODEL_MULTILINGUAL_E5_SMALL: &str = "multilingual-e5-small";

pub const DEFAULT_MODEL: &str = MODEL_MULTILINGUAL_E5_SMALL;

pub struct FastEmbedProvider {
    /// std::sync::Mutex so we can use spawn_blocking without holding an async lock.
    /// This avoids serializing all concurrent embedding requests.
    model: Arc<std::sync::Mutex<TextEmbedding>>,
    dimensions: usize,
    model_id: String,
}

/// Configured embedding model identifier (env: `COGNOS_EMBEDDING_MODEL`).
/// Falls back to `DEFAULT_MODEL` when unset. See accepted values in `resolve_model`.
pub(crate) fn model_id_from_env() -> String {
    std::env::var("COGNOS_EMBEDDING_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string())
}

fn resolve_model(
    id: &str,
) -> Result<(EmbeddingModel, usize), Box<dyn std::error::Error + Send + Sync>> {
    match id {
        MODEL_BGE_SMALL_EN => Ok((EmbeddingModel::BGESmallENV15, 384)),
        MODEL_MULTILINGUAL_E5_SMALL => Ok((EmbeddingModel::MultilingualE5Small, 384)),
        other => Err(format!(
            "Unknown COGNOS_EMBEDDING_MODEL: {other:?} (expected one of: {}, {})",
            MODEL_BGE_SMALL_EN, MODEL_MULTILINGUAL_E5_SMALL
        )
        .into()),
    }
}

impl FastEmbedProvider {
    pub fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Self::with_model(&model_id_from_env())
    }

    pub fn with_model(id: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let (model, dimensions) = resolve_model(id)?;
        let options = InitOptions::new(model).with_show_download_progress(true);
        let model = TextEmbedding::try_new(options)?;

        Ok(Self {
            model: Arc::new(std::sync::Mutex::new(model)),
            dimensions,
            model_id: id.to_string(),
        })
    }
}

#[async_trait]
impl EmbeddingProvider for FastEmbedProvider {
    async fn embed(
        &self,
        text: &str,
    ) -> Result<Vec<f32>, Box<dyn std::error::Error + Send + Sync>> {
        let model = self.model.clone();
        let text = text.to_string();
        tokio::task::spawn_blocking(move || {
            let model = model.lock().map_err(|e| format!("Mutex poisoned: {}", e))?;
            let embeddings = model.embed(vec![text], None)?;
            embeddings
                .into_iter()
                .next()
                .ok_or_else(|| "fastembed returned no embeddings".into())
        })
        .await?
    }

    async fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error + Send + Sync>> {
        let model = self.model.clone();
        let texts: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        tokio::task::spawn_blocking(move || {
            let model = model.lock().map_err(|e| format!("Mutex poisoned: {}", e))?;
            let embeddings = model.embed(texts, None)?;
            Ok(embeddings)
        })
        .await?
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    /// Identifier of the loaded model, for storage in `memory_entries.embedding_model`.
    fn model_id(&self) -> &str {
        &self.model_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared_provider() -> &'static FastEmbedProvider {
        crate::test_util::shared_embedder()
    }

    #[tokio::test]
    async fn test_embed_single_text() {
        let provider = shared_provider();
        let embedding = provider.embed("hello world").await.unwrap();

        assert_eq!(embedding.len(), provider.dimensions());
        assert!(embedding.iter().any(|&x| x != 0.0));
    }

    #[tokio::test]
    async fn test_embed_batch() {
        let provider = shared_provider();
        let embeddings = provider.embed_batch(&["hello", "world"]).await.unwrap();

        assert_eq!(embeddings.len(), 2);
        assert_eq!(embeddings[0].len(), provider.dimensions());
    }

    #[tokio::test]
    async fn test_similar_texts_have_similar_embeddings() {
        let provider = shared_provider();
        let emb1 = provider.embed("sales report for Q4").await.unwrap();
        let emb2 = provider.embed("quarterly sales analysis").await.unwrap();
        let emb3 = provider.embed("chocolate cake recipe").await.unwrap();

        let sim_related = crate::memory::cosine_similarity(&emb1, &emb2);
        let sim_unrelated = crate::memory::cosine_similarity(&emb1, &emb3);

        assert!(
            sim_related > sim_unrelated,
            "Related texts should be more similar"
        );
    }

    #[test]
    fn test_resolve_model_known_ids() {
        assert_eq!(resolve_model(MODEL_BGE_SMALL_EN).unwrap().1, 384);
        assert_eq!(resolve_model(MODEL_MULTILINGUAL_E5_SMALL).unwrap().1, 384);
    }

    #[test]
    fn test_resolve_model_unknown_id_errors() {
        let err = resolve_model("does-not-exist").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("does-not-exist"), "missing bad input: {msg}");
        assert!(
            msg.contains(MODEL_BGE_SMALL_EN),
            "missing BGE option: {msg}"
        );
        assert!(
            msg.contains(MODEL_MULTILINGUAL_E5_SMALL),
            "missing E5 option: {msg}"
        );
    }

    /// Norwegian synonyms ("pappa"/"fars" both mean "father") must encode close
    /// together in the embedding space — required for cross-language semantic
    /// search. Threshold 0.7 is empirically conservative for MultilingualE5Small.
    #[tokio::test]
    async fn test_norwegian_synonyms_have_high_similarity() {
        let provider = shared_provider();
        let pappa = provider.embed("pappa").await.unwrap();
        let fars = provider.embed("fars").await.unwrap();

        let sim = crate::memory::cosine_similarity(&pappa, &fars);
        assert!(
            sim > 0.7,
            "Expected high similarity for Norwegian synonyms, got {}",
            sim
        );
    }

    /// Cross-language semantic search: "pappa øye" must rank Norwegian text
    /// containing "fars" / "fødselsnummer" above unrelated Norwegian text.
    /// Thread search ranks by the same cosine similarity, so this is the
    /// property the search depends on.
    #[tokio::test]
    async fn test_pappa_oye_query_matches_norwegian_father_id_thread() {
        let provider = shared_provider();

        let embeddings = provider
            .embed_batch(&[
                "pappa øye",
                "Hva er fars fødselsnummer? Jeg trenger det til skattemeldingen.",
                "Oppskrift på sjokoladekake med kremost",
            ])
            .await
            .unwrap();

        let target_sim = crate::memory::cosine_similarity(&embeddings[0], &embeddings[1]);
        let unrelated_sim = crate::memory::cosine_similarity(&embeddings[0], &embeddings[2]);

        assert!(
            target_sim > 0.75,
            "Expected 'pappa øye' to have non-trivial similarity to Norwegian \
             father/fødselsnummer text, got {} (unrelated baseline {})",
            target_sim,
            unrelated_sim
        );
        assert!(
            target_sim > unrelated_sim,
            "Expected 'pappa øye' to rank Norwegian father-text ABOVE unrelated \
             Norwegian cake-text. target={} unrelated={}",
            target_sim,
            unrelated_sim
        );
    }
}
