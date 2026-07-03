//! Late-binding holder for the shared [`FastEmbedProvider`].
//!
//! The embedding model is a multi-hundred-MB HuggingFace download on a cold
//! cache. A packaged first run that is offline (or HF-blocked) must still
//! BOOT — a fatal `FastEmbedProvider::new()?` at construction left the
//! workspace in a gateway respawn loop with nothing but the boot splash. So
//! construction fills this slot when the model loads, or leaves it EMPTY on a
//! *fetch-class* failure (`fastembed::is_model_fetch_failure`; corrupt-model
//! errors stay fatal) and a background task retries with backoff
//! (`LucidosEngine::spawn_embedder_retry_if_degraded`), installing the
//! provider without a restart once the download succeeds.
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
    /// Slot holding an already-loaded provider (the normal warm-cache boot).
    pub fn ready(provider: FastEmbedProvider) -> Arc<Self> {
        Arc::new(Self {
            model_id: provider.model_id().to_string(),
            inner: RwLock::new(Some(Arc::new(provider))),
        })
    }

    /// Empty slot — the degraded boot. Embeds error with
    /// [`EMBEDDER_UNAVAILABLE`] until [`Self::install`] fills it.
    pub fn empty() -> Arc<Self> {
        Arc::new(Self {
            model_id: model_id_from_env(),
            inner: RwLock::new(None),
        })
    }

    /// Install a late-loaded provider (the background retry succeeding).
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

/// The boot decision (engine construction): a loaded provider fills the slot;
/// a *fetch-class* init failure boots DEGRADED with an empty slot (the
/// background retry recovers it); any other init error stays FATAL — a
/// corrupt cached model or config bug must not boot a silently memory-less
/// engine. Pure over the init `Result` so the three arms are unit-testable
/// without a model download or a full engine.
pub fn slot_from_init(
    result: Result<FastEmbedProvider, Box<dyn std::error::Error + Send + Sync>>,
) -> Result<Arc<EmbedderSlot>, Box<dyn std::error::Error + Send + Sync>> {
    match result {
        Ok(provider) => Ok(EmbedderSlot::ready(provider)),
        Err(e) if super::fastembed::is_model_fetch_failure(e.as_ref()) => {
            crate::log!(
                "[Memory] Embedding model unavailable at boot (fetch failed): {} — \
                 booting with memory features disabled; retrying in the background",
                e
            );
            Ok(EmbedderSlot::empty())
        }
        Err(e) => Err(e),
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

    /// A fetch-class init failure (offline first run) must BOOT — empty slot,
    /// not an error. This is the invariant that un-bricks a packaged install
    /// with no network: before this, `FastEmbedProvider::new()?` killed
    /// construction and the gateway respawn-looped on the boot splash.
    #[test]
    fn fetch_failure_boots_degraded() {
        // Message shape mirrors fastembed's fetch-path wrapper + HF transport.
        let err: Box<dyn std::error::Error + Send + Sync> =
            "Failed to retrieve onnx/model.onnx: request error: https://huggingface.co/…: \
             connection refused"
                .into();
        let slot = slot_from_init(Err(err)).expect("fetch failure must boot degraded");
        assert!(!slot.is_ready(), "degraded boot leaves the slot empty");
    }

    /// A NON-fetch init failure (corrupt cached model, bad config) must stay
    /// FATAL — booting a silently memory-less engine would hide a real bug.
    #[test]
    fn non_fetch_failure_stays_fatal() {
        let err: Box<dyn std::error::Error + Send + Sync> =
            "could not read model.onnx file: invalid protobuf".into();
        let e = match slot_from_init(Err(err)) {
            Err(e) => e,
            Ok(_) => panic!("non-fetch init failure must stay fatal"),
        };
        assert!(e.to_string().contains("could not read"), "{e}");
    }

    /// An empty slot must degrade DESCRIPTIVELY, never panic or return empty
    /// results as success — the no-hidden-errors contract for a first boot
    /// whose model download hasn't landed yet.
    #[tokio::test]
    async fn empty_slot_errors_descriptively() {
        let slot = EmbedderSlot::empty();
        assert!(!slot.is_ready());
        let err = slot.embed("hello").await.expect_err("empty slot must error");
        assert!(
            err.to_string().contains("embedding model is not available"),
            "error must explain the degraded state: {err}"
        );
        let err = slot
            .embed_batch(&["a", "b"])
            .await
            .expect_err("empty slot must error");
        assert!(err.to_string().contains("retrying in the background"), "{err}");
        // model_id stays the configured id so memory rows written later (after
        // install) match what reembed/stale checks expect.
        assert!(!slot.model_id().is_empty());
        assert_eq!(slot.dimensions(), EMPTY_SLOT_DIMENSIONS);
    }
}
