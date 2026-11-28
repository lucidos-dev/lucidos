use std::sync::Arc;

use crate::engine::event_bus::SystemEvent;
use crate::engine::LucidosEngine;
use crate::memory::fastembed::{is_model_fetch_failure, FastEmbedProvider};
use crate::memory::legacy_cache::{reclaim_legacy_cache, seed_shared_cache_from_legacy};
use crate::memory::model_download::{
    cache_dir, ensure_model_cached, CacheOutcome, DownloadFrame, ModelDownloadObserver,
};
use crate::memory::provider::EmbeddingProvider;
use crate::memory::EmbeddingModelLoadState;

const RETRY_DELAYS_SECS: &[u64] = &[30, 60, 120, 300, 600];

const NOTIFY_AFTER_ATTEMPTS: u32 = 3;

const PEER_DOWNLOAD_POLL_SECS: u64 = 5;

fn delay_before_attempt(completed_attempts: u32) -> Option<std::time::Duration> {
    if completed_attempts == 0 {
        return None;
    }
    let idx = (completed_attempts - 1) as usize;
    let secs = RETRY_DELAYS_SECS
        .get(idx)
        .copied()
        .unwrap_or(*RETRY_DELAYS_SECS.last().expect("non-empty schedule"));
    Some(std::time::Duration::from_secs(secs))
}

/// INVARIANT: Sleep before the next pass of the load loop.
///
/// `peer_wait` means the previous pass stopped because another engine on this
/// machine held the download lock for the SHARED model cache. That is not a
/// failed attempt: nothing is wrong, the bytes are arriving in the very
/// directory this engine reads, and the only sane thing to do is look again
/// shortly. So it takes [`PEER_DOWNLOAD_POLL_SECS`] instead of the failure
/// backoff, and the caller leaves `completed_attempts` alone, which is what
/// keeps a parallel cold start from marching its losers to
/// [`NOTIFY_AFTER_ATTEMPTS`] and announcing a degradation that is not happening.
fn delay_before_pass(completed_attempts: u32, peer_wait: bool) -> Option<std::time::Duration> {
    if peer_wait {
        return Some(std::time::Duration::from_secs(PEER_DOWNLOAD_POLL_SECS));
    }
    delay_before_attempt(completed_attempts)
}

fn broadcast_status(engine: &Arc<LucidosEngine>) {
    let status = engine.embedder().status();
    engine
        .event_bus
        .broadcast_transient_system(SystemEvent::EmbeddingModelStatusChanged {
            model_id: status.model_id,
            load_state: status.load_state,
        });
}

fn publish(engine: &Arc<LucidosEngine>, state: EmbeddingModelLoadState) {
    engine.embedder().set_load_state(state);
    broadcast_status(engine);
}

/// INVARIANT: Whether a freshly-built provider's vector width disagrees with the live
/// `memory_entries.embedding` column, returning the message to report if so.
///
/// Both accepted model ids are 384-dim today, so this cannot fire yet. It
/// exists because nothing else checks: `VECTOR_DIM` only ever reaches a
/// `CREATE TABLE IF NOT EXISTS`, so an existing workspace's column keeps its
/// original width and a future model swap would install cleanly and then break
/// every write. The message names both numbers, because which side is wrong is
/// the whole question.
///
/// `None` when there is no memory index, or the table has not been created yet:
/// no column means nothing to disagree with.
async fn dimension_mismatch(
    engine: &Arc<LucidosEngine>,
    provider: &FastEmbedProvider,
) -> Option<String> {
    let index = engine.memory_index().clone()?;
    let column = match index.embedding_column_dimensions().await {
        Ok(dims) => dims?,
        Err(e) => {
            // INVARIANT: Could not ask. Do NOT invent a mismatch: refusing to install over
            // a transient DB hiccup would disable memory for the whole session.
            log_legacy!(@Memory, "Could not read the embedding column width, skipping the dimension check: {}", e);
            return None;
        }
    };
    let model = provider.dimensions();
    if model == column {
        return None;
    }
    Some(format!(
        "The embedding model '{}' produces {}-dimensional vectors, but this workspace's \
         memory_entries.embedding column is vector({}). Memory is disabled rather than \
         writing rows that cannot be stored. Either set LUCIDOS_EMBEDDING_MODEL back to a \
         {}-dimensional model and restart, or migrate the column and rebuild memory.",
        provider.model_id(),
        model,
        column,
        column
    ))
}

struct DownloadReporter {
    engine: Arc<LucidosEngine>,
}

impl ModelDownloadObserver for DownloadReporter {
    fn progressed(&self, frame: DownloadFrame) {
        publish(
            &self.engine,
            EmbeddingModelLoadState::Downloading {
                downloaded_bytes: frame.downloaded_bytes,
                total_bytes: frame.total_bytes,
            },
        );
    }
}

impl LucidosEngine {
    pub fn spawn_embedder_load(self: &Arc<Self>) {
        let engine = Arc::clone(self);
        // INVARIANT: Build the provider for the id the slot CAPTURED at construction, not a
        // fresh `LUCIDOS_EMBEDDING_MODEL` read: `apply_to_process_env` can change
        // that env var after the slot resolved its id, and the slot's `model_id`
        // (stamped on every row and used by `reembed_stale`) must match the model
        // actually loaded. Resolving from the slot keeps the two in lockstep.
        let model_id = engine.embedder().model_id().to_string();
        tokio::spawn(async move {
            let mut attempts: u32 = 0;
            let mut peer_wait = false;
            let mut notified_degraded = false;
            loop {
                if let Some(delay) = delay_before_pass(attempts, peer_wait) {
                    tokio::time::sleep(delay).await;
                }
                if engine.is_shutting_down() {
                    return;
                }
                peer_wait = false;
                let attempt = attempts + 1;
                publish(&engine, EmbeddingModelLoadState::Loading);
                let id = model_id.clone();
                let reporter = DownloadReporter {
                    engine: Arc::clone(&engine),
                };
                let load_engine = Arc::clone(&engine);
                let workspace = engine.workspace_path().to_path_buf();
                let runtime = tokio::runtime::Handle::current();
                // INVARIANT: Download and ONNX load share one blocking thread. Splitting
                // the fetch out of `with_model` is what buys byte progress:
                // fastembed offers no hook, so the files are pulled first (with
                // one) and `with_model` then finds them local. The two
                // legacy-cache steps are blocking filesystem work too, so they
                // belong on this thread rather than the runtime's. The fetch
                // itself is async and deadline-bounded, so it always returns.
                //
                // `None` means a peer holds the lock: the cache is not complete,
                // so there is nothing to load yet and `with_model` would only
                // queue behind the same lock.
                type LoadResult =
                    Result<Option<FastEmbedProvider>, Box<dyn std::error::Error + Send + Sync>>;
                match tokio::task::spawn_blocking(move || -> LoadResult {
                    let active_cache = cache_dir();
                    seed_shared_cache_from_legacy(&workspace, &active_cache);

                    let outcome = runtime.block_on(ensure_model_cached(&id, &reporter))?;
                    if outcome == CacheOutcome::PeerDownloading {
                        return Ok(None);
                    }
                    if outcome == CacheOutcome::Downloaded {
                        publish(&load_engine, EmbeddingModelLoadState::Loading);
                    }
                    let provider = FastEmbedProvider::with_model(&id)?;
                    // INVARIANT: A built provider is the proof that the active cache is
                    // complete and usable, which is exactly what makes any
                    // per-workspace copy safe to drop.
                    reclaim_legacy_cache(&workspace, &active_cache);
                    Ok(Some(provider))
                })
                .await
                {
                    Ok(Ok(None)) => {
                        peer_wait = true;
                        log_legacy!(
                            @Memory,
                            "Another engine is downloading the embedding model into the shared \
                             cache; checking again in {}s",
                            PEER_DOWNLOAD_POLL_SECS
                        );
                    }
                    Ok(Ok(Some(provider))) => {
                        // INVARIANT: The model loads fine but may not FIT: an existing
                        // workspace's vector column keeps the width it was
                        // created with. Refuse before installing, or every
                        // insert and every re-embed UPDATE fails on a
                        // dimension mismatch with nothing said out loud.
                        if let Some(message) = dimension_mismatch(&engine, &provider).await {
                            log_legacy!(
                                "[Memory] {}. Giving up; memory stays disabled until the model \
                                 or the column changes",
                                message
                            );
                            publish(
                                &engine,
                                EmbeddingModelLoadState::Failed {
                                    message: message.clone(),
                                },
                            );
                            notify(&engine, "Memory features are disabled", &message).await;
                            return;
                        }
                        engine.embedder().install(provider);
                        broadcast_status(&engine);
                        log_legacy!(
                            "[Memory] Embedding model loaded (attempt #{}) — memory features active",
                            attempt
                        );
                        if let Some(index) = engine.memory_index().clone() {
                            let embedder = engine.embedder().clone();
                            if let Err(e) =
                                crate::memory::reembed::reembed_stale(&index, embedder).await
                            {
                                log_legacy!(@Memory, "Re-embed task failed: {}", e);
                            }
                        }
                        if notified_degraded {
                            notify(
                                &engine,
                                "Memory is ready",
                                "The embedding model finished downloading — memory search, \
                                 extraction, and semantic thread search are now active.",
                            )
                            .await;
                        }
                        return;
                    }
                    Ok(Err(e)) if is_model_fetch_failure(e.as_ref()) => {
                        attempts = attempt;
                        log_legacy!(
                            "[Memory] Embedding model load attempt #{} failed (fetch): {} — retrying in the background",
                            attempt,
                            e
                        );
                        publish(&engine, EmbeddingModelLoadState::Waiting { attempt });
                        if attempt == NOTIFY_AFTER_ATTEMPTS {
                            notify(
                                &engine,
                                "Memory features are waiting on a download",
                                "The embedding model (~465 MB from huggingface.co) hasn't \
                                 downloaded yet — memory search and extraction are disabled \
                                 until it lands. Lucidos keeps retrying in the background; \
                                 check the machine's internet access if this persists.",
                            )
                            .await;
                            notified_degraded = true;
                        }
                    }
                    Ok(Err(e)) => {
                        log_legacy!(
                            "[Memory] Embedding model init failed with a NON-network error: {} — \
                             giving up (fix the model cache / config and restart)",
                            e
                        );
                        publish(
                            &engine,
                            EmbeddingModelLoadState::Failed {
                                message: e.to_string(),
                            },
                        );
                        notify(
                            &engine,
                            "Memory features are disabled",
                            &format!(
                                "The embedding model failed to initialize with a non-network \
                                 error and automatic retries stopped: {e}. Clearing the \
                                 fastembed cache directory and restarting usually fixes a \
                                 corrupt download."
                            ),
                        )
                        .await;
                        return;
                    }
                    Err(join_err) => {
                        attempts = attempt;
                        log_legacy!(
                            "[Memory] Embedding model load task panicked: {} — retrying",
                            join_err
                        );
                        publish(&engine, EmbeddingModelLoadState::Waiting { attempt });
                    }
                }
            }
        });
    }
}

async fn notify(engine: &Arc<LucidosEngine>, title: &str, message: &str) {
    if let Err(e) = engine
        .create_notification(
            title,
            message,
            None,
            None,
            None,
            crate::scheduler::notifications::Tap::Modal,
            None,
        )
        .await
    {
        log_legacy!("[Memory] Failed to emit embedder notification: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn first_attempt_is_immediate_then_backs_off() {
        assert_eq!(
            delay_before_attempt(0),
            None,
            "the first attempt must not sleep"
        );
        assert_eq!(delay_before_attempt(1), Some(Duration::from_secs(30)));
        assert_eq!(delay_before_attempt(2), Some(Duration::from_secs(60)));
        assert_eq!(delay_before_attempt(3), Some(Duration::from_secs(120)));
        let last = *RETRY_DELAYS_SECS.last().unwrap();
        assert_eq!(
            delay_before_attempt(RETRY_DELAYS_SECS.len() as u32),
            Some(Duration::from_secs(last))
        );
        assert_eq!(
            delay_before_attempt(999),
            Some(Duration::from_secs(last)),
            "backoff clamps to the last entry, never panics"
        );
    }

    #[test]
    fn a_peer_wait_polls_briefly_and_does_not_advance_the_backoff() {
        assert_eq!(
            delay_before_pass(0, true),
            Some(Duration::from_secs(PEER_DOWNLOAD_POLL_SECS)),
            "a peer wait polls, even before any real attempt has completed"
        );
        assert_eq!(
            delay_before_pass(2, true),
            Some(Duration::from_secs(PEER_DOWNLOAD_POLL_SECS))
        );
        assert!(
            PEER_DOWNLOAD_POLL_SECS < RETRY_DELAYS_SECS[0],
            "a peer wait must be shorter than the first failure backoff, or the \
             loser lags the winner by minutes for no reason"
        );
        assert_eq!(delay_before_pass(0, false), None);
        assert_eq!(delay_before_pass(2, false), delay_before_attempt(2));
    }
}
