//! Late-binding holder for the shared [`FastEmbedProvider`].
//!
//! The embedding model is a multi-hundred-MB HuggingFace download on a cold
//! cache (and a non-trivial ONNX load even when warm), so it must NEVER block
//! boot — a workspace should open immediately regardless of the model.
//! Construction ALWAYS fills this slot [`EmbedderSlot::empty`], and the
//! background loader (`LucidosEngine::spawn_embedder_load`) loads the model —
//! trying immediately, then with backoff on a *fetch-class* failure
//! (`fastembed::is_model_fetch_failure`) — and [`EmbedderSlot::install`]s the
//! provider into the live slot without a restart once it lands. A non-fetch
//! failure (corrupt cached model) stops retrying and disables memory with a loud
//! notification, but never crashes boot.
//!
//! The slot implements [`EmbeddingProvider`] itself: while empty, every embed
//! call returns the descriptive [`EMBEDDER_UNAVAILABLE`] error, which the
//! existing consumers already surface (memory tools report it, thread search
//! degrades to text-only, context building logs and skips recall). No
//! consumer needs to know about the late binding.

use async_trait::async_trait;
use std::sync::{Arc, RwLock};

use super::fastembed::{model_id_from_env, FastEmbedProvider};
use super::provider::EmbeddingProvider;

/// Error every embed call returns while the model hasn't loaded yet. Names the
/// cause and the self-healing so the message is actionable wherever it
/// surfaces (a tool result, a log line, an API error).
pub const EMBEDDER_UNAVAILABLE: &str = "the embedding model is not available yet — its first-run \
     download from huggingface.co has not succeeded (offline, or the host is blocked). Memory \
     search/extraction and semantic thread search are disabled until it lands; the engine keeps \
     retrying in the background and recovers without a restart";

/// Both supported fastembed models embed at 384 dims; used for
/// `dimensions()` while the slot is empty (no caller allocates off it before
/// a successful `embed`, but the trait method must answer something truthful).
const EMPTY_SLOT_DIMENSIONS: usize = 384;

pub struct EmbedderSlot {
    inner: RwLock<Option<Arc<FastEmbedProvider>>>,
    /// The *configured* model id (env resolution) so `model_id()` is stable
    /// and truthful before, during, and after the late load — `with_model`
    /// builds the provider from the same id.
    model_id: String,
}

impl EmbedderSlot {
    /// Empty slot — the boot state. Embeds error with [`EMBEDDER_UNAVAILABLE`]
    /// until the background loader ([`Self::install`]) fills it.
    pub fn empty() -> Arc<Self> {
        Arc::new(Self {
            model_id: model_id_from_env(),
            inner: RwLock::new(None),
        })
    }

    /// Install a late-loaded provider (the background loader succeeding).
    pub fn install(&self, provider: FastEmbedProvider) {
        *self.inner.write().expect("EmbedderSlot lock poisoned") = Some(Arc::new(provider));
    }

    pub fn is_ready(&self) -> bool {
        self.inner
            .read()
            .expect("EmbedderSlot lock poisoned")
            .is_some()
    }

    /// Clone the inner provider out under a short read guard (never held
    /// across an `.await`) — mirrors the engine's swappable-LLM convention.
    fn get(&self) -> Option<Arc<FastEmbedProvider>> {
        self.inner
            .read()
            .expect("EmbedderSlot lock poisoned")
            .clone()
    }
}

#[async_trait]
impl EmbeddingProvider for EmbedderSlot {
    async fn embed(
        &self,
        text: &str,
    ) -> Result<Vec<f32>, Box<dyn std::error::Error + Send + Sync>> {
        let Some(provider) = self.get() else {
            return Err(EMBEDDER_UNAVAILABLE.into());
        };
        provider.embed(text).await
    }

    async fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error + Send + Sync>> {
        let Some(provider) = self.get() else {
            return Err(EMBEDDER_UNAVAILABLE.into());
        };
        provider.embed_batch(texts).await
    }

    fn dimensions(&self) -> usize {
        self.get()
            .map(|p| p.dimensions())
            .unwrap_or(EMPTY_SLOT_DIMENSIONS)
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty slot must degrade DESCRIPTIVELY, never panic or return empty
    /// results as success — the no-hidden-errors contract for a first boot
    /// whose model download hasn't landed yet.
    #[tokio::test]
    async fn empty_slot_errors_descriptively() {
        let slot = EmbedderSlot::empty();
        assert!(!slot.is_ready());
        let err = slot
            .embed("hello")
            .await
            .expect_err("empty slot must error");
        assert!(
            err.to_string().contains("embedding model is not available"),
            "error must explain the degraded state: {err}"
        );
        let err = slot
            .embed_batch(&["a", "b"])
            .await
            .expect_err("empty slot must error");
        assert!(
            err.to_string().contains("retrying in the background"),
            "{err}"
        );
        // model_id stays the configured id so memory rows written later (after
        // install) match what reembed/stale checks expect.
        assert!(!slot.model_id().is_empty());
        assert_eq!(slot.dimensions(), EMPTY_SLOT_DIMENSIONS);
    }
}
