use async_trait::async_trait;

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, text: &str)
        -> Result<Vec<f32>, Box<dyn std::error::Error + Send + Sync>>;

    async fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error + Send + Sync>>;

    fn dimensions(&self) -> usize;

    fn model_id(&self) -> &str;
}
