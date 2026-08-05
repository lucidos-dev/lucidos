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
    /// std::sync::Mutex (not a tokio one) so the inference can run inside
    /// `spawn_blocking` without a lock ever being held across an `.await`.
    /// Concurrent embed calls DO serialize on it: there is one `TextEmbedding`
    /// session and `embed` takes `&self` on it for the whole inference.
    model: Arc<std::sync::Mutex<TextEmbedding>>,
    dimensions: usize,
    model_id: String,
}

/// Configured embedding model identifier (env: `LUCIDOS_EMBEDDING_MODEL`).
/// Falls back to `DEFAULT_MODEL` when unset. See accepted values in `resolve_model`.
pub(crate) fn model_id_from_env() -> String {
    std::env::var("LUCIDOS_EMBEDDING_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string())
}

/// True when a `FastEmbedProvider::new()` error is a model-download / network
/// failure (HF unreachable, request timed out, too many retries) rather than a
/// logic / data bug. Decides two things: whether a real-embedder test may SKIP
/// (`test_util::shared_embedder`), and whether engine construction may boot
/// DEGRADED with an empty [`super::EmbedderSlot`] instead of failing — a
/// packaged first run with no network must still boot (the model retries in
/// the background), while a corrupt cached model must stay a loud, fatal bug.
///
/// The match walks the **entire error source chain**, not just the top
/// message: `fastembed` wraps the underlying transport error with anyhow
/// context like `Failed to retrieve onnx/model.onnx` (preserved as a
/// `.source()`), so the network signal lives one level down. The raw HF
/// errors look like `request error: <url>: <ureq error>` / `Too many
/// retries: …`, and the HF host appears in every fetch URL. `failed to
/// retrieve` is fastembed's own *fetch*-path wrapper — distinct from the
/// `could not read … file` messages it emits when an already-downloaded file
/// is corrupt, which we deliberately do NOT match so real corruption still
/// fails loudly.
pub fn is_model_fetch_failure(err: &(dyn std::error::Error + Send + Sync)) -> bool {
    const MARKERS: &[&str] = &[
        "huggingface",
        "request error",
        "network error",
        "timed out",
        "timeout",
        "too many retries",
        "connection",
        "failed to lookup",
        "dns error",
        "no such host",
        "failed to retrieve", // fastembed's fetch-path context wrapper
    ];
    let mut level: Option<&dyn std::error::Error> = Some(err);
    while let Some(e) = level {
        let msg = e.to_string().to_lowercase();
        if MARKERS.iter().any(|needle| msg.contains(needle)) {
            return true;
        }
        level = e.source();
    }
    false
}

/// Map a configured model id onto the fastembed variant and its vector width.
/// `pub(crate)` because `model_download` resolves the same id to find the repo
/// and file set it has to pre-fetch, and the two must never disagree about
/// which model an id names.
pub(crate) fn resolve_model(
    id: &str,
) -> Result<(EmbeddingModel, usize), Box<dyn std::error::Error + Send + Sync>> {
    match id {
        MODEL_BGE_SMALL_EN => Ok((EmbeddingModel::BGESmallENV15, 384)),
        MODEL_MULTILINGUAL_E5_SMALL => Ok((EmbeddingModel::MultilingualE5Small, 384)),
        other => Err(format!(
            "Unknown LUCIDOS_EMBEDDING_MODEL: {other:?} (expected one of: {}, {})",
            MODEL_BGE_SMALL_EN, MODEL_MULTILINGUAL_E5_SMALL
        )
        .into()),
    }
}

/// Actionable wrapper for a model-init failure: the `TextEmbedding::try_new`
/// load, and the [`model_download`](super::model_download) fetch that now runs
/// ahead of it. `cause` is the flattened (`{e:#}`) original error chain.
///
/// Shared by both so a cold-cache failure keeps naming the cache directory and
/// the CA bundle whichever half of the load hit it. Splitting the download out
/// of `with_model` would otherwise have quietly dropped that guidance from
/// exactly the case it was written for.
///
/// The guidance text deliberately contains NONE of the
/// [`is_model_fetch_failure`] markers (say "the Hugging Face model hub", never
/// the `huggingface` token; no "connection"/"timeout"/"download" phrasing that
/// overlaps a marker): classification must key ONLY on the original cause
/// baked into `cause`, or every init failure — a corrupt cached model included
/// — would classify as a fetch failure and boot degraded-with-retries instead
/// of failing loudly. Pinned by `init_error_message_*` tests below.
pub(crate) fn init_error_message(
    id: &str,
    cause: String,
) -> Box<dyn std::error::Error + Send + Sync> {
    // Name the whole resolution ORDER rather than one env var. `fastembed`
    // reads `HF_HOME` first (see `model_download::cache_dir`, which mirrors its
    // `pull_from_hf`), so reporting only `FASTEMBED_CACHE_DIR` sent a user with
    // `HF_HOME` set to pre-seed a directory fastembed never looks in. The
    // default is also CWD-relative, not workspace-relative. Interpolating the
    // resolved path is deliberately avoided on top of that: this string is what
    // `is_model_fetch_failure` classifies on, and a cache path containing a
    // marker word would make every init failure read as a fetch failure.
    format!(
        "embedding model '{id}' failed to initialize: {cause}. First boot fetches it from the \
         Hugging Face model hub, so check outbound HTTPS (and the system CA bundle), or pre-seed \
         the model cache from a machine that already has it. The cache directory is $HF_HOME when \
         set, else $FASTEMBED_CACHE_DIR, else .fastembed_cache relative to the engine's working \
         directory."
    )
    .into()
}

impl FastEmbedProvider {
    pub fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Self::with_model(&model_id_from_env())
    }

    /// Build a provider for an explicit model id. The background loader passes
    /// the [`EmbedderSlot`](super::embedder_slot::EmbedderSlot)'s captured id
    /// (rather than re-reading `LUCIDOS_EMBEDDING_MODEL`) so the loaded model
    /// always matches the id stamped on rows — `apply_to_process_env` can change
    /// the env var between construction and the background load.
    pub fn with_model(id: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let (model, dimensions) = resolve_model(id)?;
        let options = InitOptions::new(model).with_show_download_progress(true);
        // A failed init surfaces on the boot path (a fetch-class failure boots
        // DEGRADED via `EmbedderSlot`; anything else is fatal before HTTP
        // binds) — make the failure name the model, the cache, and the two
        // remedies instead of surfacing a bare hf-hub transport error.
        // `{e:#}` (anyhow alternate) flattens the WHOLE cause chain into the
        // text: this wrap turns the error into a plain string, and
        // `is_model_fetch_failure` reads the network markers from it.
        let model = TextEmbedding::try_new(options)
            .map_err(|e| init_error_message(id, format!("{e:#}")))?;

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

    #[test]
    fn test_resolve_model_known_ids() {
        assert_eq!(resolve_model(MODEL_BGE_SMALL_EN).unwrap().1, 384);
        assert_eq!(resolve_model(MODEL_MULTILINGUAL_E5_SMALL).unwrap().1, 384);
    }

    /// The init wrapper's own guidance text must not trip the fetch
    /// classifier: a corrupt cached model ("could not read … file") wrapped by
    /// `init_error_message` has to stay NON-fetch, or it boots
    /// degraded-with-retries (and skips real-embedder tests) instead of
    /// failing loudly. Regression for the 2026-07-01 merge, where the wrapper
    /// said "huggingface.co" and made EVERY init failure classify as fetch.
    #[test]
    fn init_error_message_keeps_corrupt_model_non_fetch() {
        let err = init_error_message(
            DEFAULT_MODEL,
            "could not read model.onnx file: invalid protobuf".to_string(),
        );
        assert!(
            !is_model_fetch_failure(err.as_ref()),
            "wrapper guidance text must not carry fetch markers: {err}"
        );
    }

    /// The inverse direction: a genuine transport failure keeps classifying as
    /// fetch THROUGH the wrapper, because `{e:#}` bakes the original cause
    /// (request error / hf URL) into the message.
    #[test]
    fn init_error_message_keeps_transport_failure_fetch_class() {
        let err = init_error_message(
            DEFAULT_MODEL,
            "Failed to retrieve onnx/model.onnx: request error: \
             https://huggingface.co/…: connection refused"
                .to_string(),
        );
        assert!(
            is_model_fetch_failure(err.as_ref()),
            "the flattened original cause must still classify as fetch: {err}"
        );
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

    // The tests below exercise the real fastembed model. They download
    // ~465 MB from huggingface.co on first run, so they're gated behind the
    // `real-embedder-tests` Cargo feature to keep default `cargo test`
    // offline. Run with:
    //     cargo test -p lucidos-engine --features real-embedder-tests
    //
    // Each test asks `shared_embedder()` for the model and early-returns (skips,
    // logging a SKIP line) when it's `None` — i.e. huggingface.co was
    // unreachable on a cold cache. A transient HF outage therefore degrades
    // these tests to skipped, never failed. When the model IS available (warm
    // `.fastembed_cache` or a successful download) every assertion below runs
    // unchanged. Assertion failures are NOT skipped — only the model-fetch path.

    #[cfg(feature = "real-embedder-tests")]
    #[tokio::test]
    async fn test_embed_single_text() {
        let Some(provider) = crate::test_util::shared_embedder() else {
            return;
        };
        let embedding = provider.embed("hello world").await.unwrap();

        assert_eq!(embedding.len(), provider.dimensions());
        assert!(embedding.iter().any(|&x| x != 0.0));
    }

    #[cfg(feature = "real-embedder-tests")]
    #[tokio::test]
    async fn test_embed_batch() {
        let Some(provider) = crate::test_util::shared_embedder() else {
            return;
        };
        let embeddings = provider.embed_batch(&["hello", "world"]).await.unwrap();

        assert_eq!(embeddings.len(), 2);
        assert_eq!(embeddings[0].len(), provider.dimensions());
    }

    #[cfg(feature = "real-embedder-tests")]
    #[tokio::test]
    async fn test_similar_texts_have_similar_embeddings() {
        let Some(provider) = crate::test_util::shared_embedder() else {
            return;
        };
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

    /// Norwegian synonyms ("bil"/"kjøretøy" both mean "vehicle") must encode close
    /// together in the embedding space — required for cross-language semantic
    /// search. Threshold 0.7 is empirically conservative for MultilingualE5Small.
    #[cfg(feature = "real-embedder-tests")]
    #[tokio::test]
    async fn test_norwegian_synonyms_have_high_similarity() {
        let Some(provider) = crate::test_util::shared_embedder() else {
            return;
        };
        let bil = provider.embed("bil").await.unwrap();
        let kjoretoy = provider.embed("kjøretøy").await.unwrap();

        let sim = crate::memory::cosine_similarity(&bil, &kjoretoy);
        assert!(
            sim > 0.7,
            "Expected high similarity for Norwegian synonyms, got {}",
            sim
        );
    }

    /// Cross-language semantic search: "bil verksted" must rank Norwegian text
    /// containing "kjøretøy" / "service" above unrelated Norwegian text.
    /// Thread search ranks by the same cosine similarity, so this is the
    /// property the search depends on.
    #[cfg(feature = "real-embedder-tests")]
    #[tokio::test]
    async fn test_bil_verksted_query_matches_norwegian_vehicle_service_thread() {
        let Some(provider) = crate::test_util::shared_embedder() else {
            return;
        };

        let embeddings = provider
            .embed_batch(&[
                "bil verksted",
                "Når er kjøretøyet ferdig på service? Jeg trenger det til arbeidsuken.",
                "Oppskrift på sjokoladekake med kremost",
            ])
            .await
            .unwrap();

        let target_sim = crate::memory::cosine_similarity(&embeddings[0], &embeddings[1]);
        let unrelated_sim = crate::memory::cosine_similarity(&embeddings[0], &embeddings[2]);

        assert!(
            target_sim > 0.75,
            "Expected 'bil verksted' to have non-trivial similarity to Norwegian \
             vehicle/service text, got {} (unrelated baseline {})",
            target_sim,
            unrelated_sim
        );
        assert!(
            target_sim > unrelated_sim,
            "Expected 'bil verksted' to rank Norwegian vehicle-text ABOVE unrelated \
             Norwegian cake-text. target={} unrelated={}",
            target_sim,
            unrelated_sim
        );
    }
}
