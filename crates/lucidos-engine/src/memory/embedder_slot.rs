//! Late-binding holder for the shared [`FastEmbedProvider`].
//!
//! The embedding model is a multi-hundred-MB HuggingFace download on a cold
//! cache (and a non-trivial ONNX load even when warm), so it must NEVER block
//! boot — a workspace should open immediately regardless of the model.
//! Construction ALWAYS fills this slot [`EmbedderSlot::empty`], and the
//! background loader (`LucidosEngine::spawn_embedder_load`) loads the model —
//! trying immediately, then with backoff on a *fetch-class* failure
//! (`fastembed::is_model_fetch_failure`) — and [`EmbedderSlot::install`]s the
//! provider into the live slot without a restart once it lands. A terminal
//! failure (a corrupt cached model, or a model whose vector width does not fit
//! this workspace's `memory_entries.embedding` column) stops retrying and
//! disables memory with a loud notification, but never crashes boot.
//!
//! The slot also carries [`EmbeddingModelLoadState`], so how far the loader has
//! got is readable from one place rather than inferred. That is what the status
//! endpoint serves and the `EmbeddingModelStatusChanged` frames narrate, and it
//! is why an installed provider cannot still claim to be loading.
//!
//! The slot implements [`EmbeddingProvider`] itself: while empty, every embed
//! call returns a descriptive error, which the existing consumers already
//! surface (memory tools report it, thread search degrades to text-only,
//! context building logs and skips recall). Which error depends on whether the
//! loader is still trying: [`EMBEDDER_UNAVAILABLE`] while it is, the terminal
//! reason once it has given up. No consumer needs to know about the late
//! binding.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

use super::fastembed::{model_id_from_env, FastEmbedProvider};
use super::provider::EmbeddingProvider;

/// Where the background loader has got to with the embedding model. Lives on
/// the slot rather than beside it so "the embedder is installed" and "the
/// loader says it is still downloading" cannot disagree: [`EmbedderSlot::install`]
/// is the only way to fill the slot and it sets [`Ready`](Self::Ready) itself.
///
/// In-memory only. A restart simply re-runs the load and re-derives this, so it
/// is runtime status, not durable state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum EmbeddingModelLoadState {
    /// Cache is cold: bytes are being fetched from the model hub. `total_bytes`
    /// is what is known so far (see `model_download::DownloadFrame`).
    Downloading {
        downloaded_bytes: u64,
        total_bytes: u64,
    },
    /// Every file is local; the ONNX session is being built. Seconds, not
    /// minutes. Also the state a fresh boot starts in, before the loader has
    /// found out whether anything needs downloading, and the state it holds
    /// while ANOTHER engine on this machine downloads into the shared model
    /// cache (nothing is failing and no backoff is in effect, so `Waiting` would
    /// be the wrong reading; see `embedder_retry::delay_before_pass`).
    Loading,
    /// Installed. Memory search, extraction and semantic thread search are live.
    Ready,
    /// A fetch-class failure (offline, hub blocked). The loader is backing off
    /// and will try again; `attempt` counts the tries so far.
    Waiting { attempt: u32 },
    /// Terminal. The loader has stopped trying, and only a fix plus a restart
    /// will change that.
    Failed { message: String },
}

impl EmbeddingModelLoadState {
    /// Whether the loader has stopped for good. Callers use this to tell a
    /// temporary degradation ("not yet") from a permanent one ("not going to").
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }
}

/// The embedding model's current status, as both the REST snapshot
/// (`GET /api/v1/memory/embedding-model-status`) and the
/// `EmbeddingModelStatusChanged` SSE payload. One shape for both so a client
/// that loads mid-download and one that watches the stream read the same thing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingModelStatus {
    /// Which model this is about, so the reading is unambiguous after a config
    /// change.
    pub model_id: String,
    pub load_state: EmbeddingModelLoadState,
}

/// Error every embed call returns while the model hasn't loaded yet. Names the
/// cause and the self-healing so the message is actionable wherever it
/// surfaces (a tool result, a log line, an API error).
///
/// Only honest while the loader is still trying. Once it has given up (a
/// corrupt cached model, a dimension mismatch) the retry promise is a lie, so
/// [`EmbedderSlot`] reports the terminal reason instead: see
/// [`EmbeddingModelLoadState::is_terminal`] and [`unavailable_reason`].
pub const EMBEDDER_UNAVAILABLE: &str = "the embedding model is not available yet — its first-run \
     download from huggingface.co has not succeeded (offline, or the host is blocked). Memory \
     search/extraction and semantic thread search are disabled until it lands; the engine keeps \
     retrying in the background and recovers without a restart";

/// What an embed call should say when the slot is empty, given where the loader
/// has got to.
///
/// The default text promises that the engine "keeps retrying in the background
/// and recovers without a restart". That is true while a download is pending or
/// backing off, and false once the loader has stopped for good, at which point
/// it sends whoever is debugging a dead memory index looking for a recovery
/// that is never coming. A terminal state reports its own reason instead.
///
/// Pure so both branches are directly testable.
fn unavailable_reason(state: &EmbeddingModelLoadState) -> String {
    match state {
        EmbeddingModelLoadState::Failed { message } => format!(
            "the embedding model is unavailable and the engine has stopped retrying: {message}"
        ),
        _ => EMBEDDER_UNAVAILABLE.to_string(),
    }
}

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
    load_state: RwLock<EmbeddingModelLoadState>,
}

impl EmbedderSlot {
    /// Empty slot — the boot state. Embeds error with [`EMBEDDER_UNAVAILABLE`]
    /// until the background loader ([`Self::install`]) fills it.
    pub fn empty() -> Arc<Self> {
        Arc::new(Self {
            model_id: model_id_from_env(),
            inner: RwLock::new(None),
            // The background loader is spawned during boot and starts work
            // immediately, so "loading" is true from the first moment anyone
            // can observe this. It becomes `Downloading` only once the loader
            // finds the cache cold.
            load_state: RwLock::new(EmbeddingModelLoadState::Loading),
        })
    }

    /// Install a late-loaded provider (the background loader succeeding). Sets
    /// the load state to [`Ready`](EmbeddingModelLoadState::Ready) in the same
    /// breath: an installed provider that still reports itself as loading would
    /// leave the UI spinning forever.
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

    /// Current load state. Cheap; safe to poll from an HTTP handler.
    pub fn load_state(&self) -> EmbeddingModelLoadState {
        self.load_state
            .read()
            .expect("EmbedderSlot load-state lock poisoned")
            .clone()
    }

    /// Record where the loader has got to. Every non-`Ready` transition comes
    /// from the background loader; `Ready` is set by [`Self::install`].
    pub fn set_load_state(&self, state: EmbeddingModelLoadState) {
        *self
            .load_state
            .write()
            .expect("EmbedderSlot load-state lock poisoned") = state;
    }

    /// Snapshot for the REST endpoint and the SSE payload.
    pub fn status(&self) -> EmbeddingModelStatus {
        EmbeddingModelStatus {
            model_id: self.model_id.clone(),
            load_state: self.load_state(),
        }
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

    /// A fresh slot must already read as working. The background loader is
    /// spawned during boot, so anything that can observe the slot at all is
    /// observing a load in progress; reporting `Ready` (or nothing) here would
    /// make an empty slot look installed.
    #[test]
    fn a_fresh_slot_reports_loading_not_ready() {
        let slot = EmbedderSlot::empty();
        assert_eq!(slot.load_state(), EmbeddingModelLoadState::Loading);
        assert!(!slot.is_ready());
        assert!(!slot.load_state().is_terminal());
    }

    /// The snapshot the REST endpoint serves reads through to the live state
    /// and carries the configured model id, so a client can tell WHICH model a
    /// reading is about.
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
        // Still empty: a download in flight must not read as an installed model.
        assert!(!slot.is_ready());
    }

    /// A terminal failure must stop promising a recovery that is not coming.
    /// The default text sends whoever is debugging a dead memory index looking
    /// for a background retry that the loader has already abandoned.
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

    /// ...and every non-terminal state keeps the original text, where the
    /// retry promise is true.
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

    /// Only `Failed` is terminal. The distinction is what lets a caller tell
    /// "not yet" from "not going to", and it is what decides whether the
    /// degraded error may promise a retry.
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
