use async_trait::async_trait;

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Embed a single text string
    async fn embed(&self, text: &str)
        -> Result<Vec<f32>, Box<dyn std::error::Error + Send + Sync>>;

    /// Embed multiple texts in a batch (more efficient)
    async fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error + Send + Sync>>;

    /// Return the dimensionality of embeddings
    fn dimensions(&self) -> usize;

    /// Identifier for the embedding model — stored alongside vectors so we can
    /// detect rows that need re-embedding after a model swap.
    fn model_id(&self) -> &str;
}
