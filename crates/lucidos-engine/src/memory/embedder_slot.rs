use async_trait::async_trait;
use std::sync::{Arc, RwLock};

use super::fastembed::{model_id_from_env, FastEmbedProvider};
use super::provider::EmbeddingProvider;

pub use crate::engine::thread_events::{EmbeddingModelLoadState, EmbeddingModelStatus};

pub const EMBEDDER_UNAVAILABLE: &str = "the embedding model is not available yet — its first-run \
     download from huggingface.co has not succeeded (offline, or the host is blocked). Memory \
     search/extraction and semantic thread search are disabled until it lands; the engine keeps \
     retrying in the background and recovers without a restart";

fn unavailable_reason(state: &EmbeddingModelLoadState) -> String {
    match state {
        EmbeddingModelLoadState::Failed { message } => format!(
            "the embedding model is unavailable and the engine has stopped retrying: {message}"
        ),
        _ => EMBEDDER_UNAVAILABLE.to_string(),
    }
}

const EMPTY_SLOT_DIMENSIONS: usize = 384;

pub struct EmbedderSlot {
    inner: RwLock<Option<Arc<FastEmbedProvider>>>,
    model_id: String,
    load_state: RwLock<EmbeddingModelLoadState>,
}

impl EmbedderSlot {
    pub fn empty() -> Arc<Self> {
        Arc::new(Self {
            model_id: model_id_from_env(),
            inner: RwLock::new(None),
            load_state: RwLock::new(EmbeddingModelLoadState::Loading),
        })
    }

    pub fn install(&self, provider: FastEmbedProvider) {
        *self.inner.write().expect("EmbedderSlot lock poisoned") = Some(Arc::new(provider));
        self.set_load_state(EmbeddingModelLoadState::Ready);
    }

    pub fn is_ready(&self) -> bool {
        self.inner
            .read()
            .expect("EmbedderSlot lock poisoned")
            .is_some()
    }

    pub fn load_state(&self) -> EmbeddingModelLoadState {
        self.load_state
            .read()
            .expect("EmbedderSlot load-state lock poisoned")
            .clone()
    }

    pub fn set_load_state(&self, state: EmbeddingModelLoadState) {
        *self
            .load_state
            .write()
            .expect("EmbedderSlot load-state lock poisoned") = state;
    }

    pub fn status(&self) -> EmbeddingModelStatus {
        EmbeddingModelStatus {
            model_id: self.model_id.clone(),
            load_state: self.load_state(),
        }
    }

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
            return Err(unavailable_reason(&self.load_state()).into());
        };
        provider.embed(text).await
    }

    async fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error + Send + Sync>> {
        let Some(provider) = self.get() else {
            return Err(unavailable_reason(&self.load_state()).into());
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
        assert!(!slot.model_id().is_empty());
        assert_eq!(slot.dimensions(), EMPTY_SLOT_DIMENSIONS);
    }

    #[test]
    fn a_fresh_slot_reports_loading_not_ready() {
        let slot = EmbedderSlot::empty();
        assert_eq!(slot.load_state(), EmbeddingModelLoadState::Loading);
        assert!(!slot.is_ready());
        assert!(!slot.load_state().is_terminal());
    }

    #[test]
    fn status_reflects_the_live_load_state() {
        let slot = EmbedderSlot::empty();
        let status = slot.status();
        assert_eq!(status.model_id, slot.model_id());
        assert_eq!(status.load_state, EmbeddingModelLoadState::Loading);

        slot.set_load_state(EmbeddingModelLoadState::Downloading {
            downloaded_bytes: 12,
            total_bytes: 40,
        });
        assert_eq!(
            slot.status().load_state,
            EmbeddingModelLoadState::Downloading {
                downloaded_bytes: 12,
                total_bytes: 40,
            }
        );
        assert!(!slot.is_ready());
    }

    #[tokio::test]
    async fn a_terminal_failure_reports_its_reason_instead_of_promising_a_retry() {
        let slot = EmbedderSlot::empty();
        slot.set_load_state(EmbeddingModelLoadState::Failed {
            message: "vector width 768 does not fit vector(384)".into(),
        });

        for err in [
            slot.embed("hello")
                .await
                .expect_err("empty slot must error"),
            slot.embed_batch(&["a"])
                .await
                .expect_err("empty slot must error"),
        ] {
            let msg = err.to_string();
            assert!(
                msg.contains("stopped retrying"),
                "a terminal failure must say so: {msg}"
            );
            assert!(
                msg.contains("vector width 768 does not fit vector(384)"),
                "the reason must survive to the caller: {msg}"
            );
            assert!(
                !msg.contains("recovers without a restart"),
                "must not promise a recovery the loader has given up on: {msg}"
            );
        }
    }

    #[test]
    fn non_terminal_states_keep_the_retry_promise() {
        for state in [
            EmbeddingModelLoadState::Loading,
            EmbeddingModelLoadState::Waiting { attempt: 2 },
            EmbeddingModelLoadState::Downloading {
                downloaded_bytes: 1,
                total_bytes: 4,
            },
        ] {
            assert_eq!(
                unavailable_reason(&state),
                EMBEDDER_UNAVAILABLE,
                "{state:?} is recoverable and must keep the default text"
            );
        }
    }

    #[test]
    fn only_a_failure_is_terminal() {
        assert!(EmbeddingModelLoadState::Failed {
            message: "bad".into()
        }
        .is_terminal());
        for state in [
            EmbeddingModelLoadState::Loading,
            EmbeddingModelLoadState::Ready,
            EmbeddingModelLoadState::Waiting { attempt: 9 },
            EmbeddingModelLoadState::Downloading {
                downloaded_bytes: 0,
                total_bytes: 1,
            },
        ] {
            assert!(!state.is_terminal(), "{state:?} must not be terminal");
        }
    }
}
