//! Background loader for the embedding model (the [`EmbedderSlot`]).
//!
//! The embedding model is a multi-hundred-MB HuggingFace download on a cold
//! cache (and a non-trivial ONNX load even when warm), so engine construction
//! boots with an EMPTY slot and never waits on it (see
//! `engine_impl/construction.rs`). This task loads the model — trying
//! immediately, then with capped exponential backoff on a *fetch-class* failure
//! (offline / HF blocked) — and installs the provider into the live slot on
//! success, so memory features come online without a restart. Nothing here is
//! persistent state (a restart simply re-runs the same load), honoring engine
//! statelessness. A warm-cache boot loads within seconds and stays silent; only
//! a boot the user was told is degraded announces recovery.
//!
//! [`EmbedderSlot`]: crate::memory::EmbedderSlot

use std::sync::Arc;

use crate::engine::event_bus::SystemEvent;
use crate::engine::LucidosEngine;
use crate::memory::fastembed::{is_model_fetch_failure, FastEmbedProvider};
use crate::memory::legacy_cache::{reclaim_legacy_cache, seed_shared_cache_from_legacy};
use crate::memory::model_download::{
    cache_dir, ensure_model_cached, CacheOutcome, DownloadFrame, ModelDownloadObserver,
};
// Brings the `model_id()` trait method into scope so the loader can read the
// slot's captured id (the slot also has a private `model_id` field).
use crate::memory::provider::EmbeddingProvider;
use crate::memory::EmbeddingModelLoadState;

/// Backoff schedule between attempts AFTER the first (immediate) one; the last
/// entry repeats forever. Starts quick (a router blip on first open should heal
/// fast) and settles at 10 minutes so an offline machine isn't hammering
/// HuggingFace.
const RETRY_DELAYS_SECS: &[u64] = &[30, 60, 120, 300, 600];

/// Attempt number after which the user is told (once) that memory is degraded
/// — early enough to explain missing recall, late enough to skip transient
/// blips that heal on an early retry.
const NOTIFY_AFTER_ATTEMPTS: u32 = 3;

/// How long to wait when another engine holds the model-download lock. Short,
/// because this is a poll of the shared cache and not a retry of a failure: the
/// files appear the moment the peer's download lands.
const PEER_DOWNLOAD_POLL_SECS: u64 = 5;

/// Sleep to apply BEFORE the next load attempt, given how many attempts have
/// already completed. `None` for the first attempt (nothing done yet) so a warm
/// cache comes online immediately; then [`RETRY_DELAYS_SECS`], clamped to its
/// last entry, so an offline machine isn't hammering HuggingFace forever. Pure
/// so the immediate-first + backoff schedule is unit-testable.
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

/// Sleep before the next pass of the load loop.
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

/// Broadcast the slot's CURRENT status to connected clients, without changing
/// it. Used after [`EmbedderSlot::install`](crate::memory::EmbedderSlot::install),
/// which sets `Ready` itself.
fn broadcast_status(engine: &Arc<LucidosEngine>) {
    let status = engine.embedder().status();
    engine
        .event_bus
        .broadcast_transient_system(SystemEvent::EmbeddingModelStatusChanged {
            model_id: status.model_id,
            load_state: status.load_state,
        });
}

/// Publish where the loader has got to: onto the slot (which the REST snapshot
/// reads, for a client that arrives mid-download) and onto the bus as a
/// transient frame (for clients already watching).
///
/// Both, in that order, so the snapshot is never staler than the last frame a
/// client received.
fn publish(engine: &Arc<LucidosEngine>, state: EmbeddingModelLoadState) {
    engine.embedder().set_load_state(state);
    broadcast_status(engine);
}

/// Whether a freshly-built provider's vector width disagrees with the live
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
            // Could not ask. Do NOT invent a mismatch: refusing to install over
            // a transient DB hiccup would disable memory for the whole session.
            log!(@Memory, "Could not read the embedding column width, skipping the dimension check: {}", e);
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

/// Turns `ensure_model_cached`'s byte frames into published `Downloading`
/// states. The frames are already throttled and monotonic (see
/// `memory::model_download`), so this adds no gating of its own.
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
    /// Spawn the background embedding-model load. Always runs (the slot boots
    /// empty); called from `main.rs` once the engine is assembled — the task
    /// needs the event bus for notifications and the memory index for the
    /// post-install re-embed sweep.
    pub fn spawn_embedder_load(self: &Arc<Self>) {
        let engine = Arc::clone(self);
        // Build the provider for the id the slot CAPTURED at construction, not a
        // fresh `LUCIDOS_EMBEDDING_MODEL` read: `apply_to_process_env` can change
        // that env var after the slot resolved its id, and the slot's `model_id`
        // (stamped on every row and used by `reembed_stale`) must match the model
        // actually loaded. Resolving from the slot keeps the two in lockstep.
        let model_id = engine.embedder().model_id().to_string();
        tokio::spawn(async move {
            // Real attempts COMPLETED. A pass that only found a peer holding the
            // download lock does not count as one: see `delay_before_pass`.
            let mut attempts: u32 = 0;
            let mut peer_wait = false;
            // Whether the user has been told memory is degraded/waiting. Gates
            // the "Memory is ready" notification so a healthy warm-cache boot
            // (which loads on the first attempt) stays silent — no per-boot
            // notification noise.
            let mut notified_degraded = false;
            loop {
                // Immediate first attempt; back off only AFTER a failure so a
                // warm cache comes online within seconds.
                if let Some(delay) = delay_before_pass(attempts, peer_wait) {
                    tokio::time::sleep(delay).await;
                }
                if engine.is_shutting_down() {
                    return;
                }
                peer_wait = false;
                let attempt = attempts + 1;
                // Each attempt starts by declaring itself, which is what takes
                // the UI out of a previous attempt's `Waiting`.
                publish(&engine, EmbeddingModelLoadState::Loading);
                let id = model_id.clone();
                let reporter = DownloadReporter {
                    engine: Arc::clone(&engine),
                };
                let load_engine = Arc::clone(&engine);
                let workspace = engine.workspace_path().to_path_buf();
                // Download and ONNX load share one blocking thread. Splitting
                // the fetch out of `with_model` is what buys byte progress:
                // fastembed offers no hook, so the files are pulled first (with
                // one) and `with_model` then finds them local. The two
                // legacy-cache steps are blocking filesystem work too, so they
                // belong on this thread rather than the runtime's.
                //
                // `None` means a peer holds the lock: the cache is not complete,
                // so there is nothing to load yet and `with_model` would only
                // queue behind the same lock.
                type LoadResult =
                    Result<Option<FastEmbedProvider>, Box<dyn std::error::Error + Send + Sync>>;
                match tokio::task::spawn_blocking(move || -> LoadResult {
                    let active_cache = cache_dir();
                    // Free upgrade: a workspace still carrying its own copy from
                    // the per-workspace era donates it to the shared cache
                    // instead of everyone re-downloading the same model.
                    seed_shared_cache_from_legacy(&workspace, &active_cache);

                    let outcome = ensure_model_cached(&id, &reporter)?;
                    if outcome == CacheOutcome::PeerDownloading {
                        return Ok(None);
                    }
                    if outcome == CacheOutcome::Downloaded {
                        // Bytes are in; the ONNX session build is what is left.
                        publish(&load_engine, EmbeddingModelLoadState::Loading);
                    }
                    let provider = FastEmbedProvider::with_model(&id)?;
                    // A built provider is the proof that the active cache is
                    // complete and usable, which is exactly what makes any
                    // per-workspace copy safe to drop.
                    reclaim_legacy_cache(&workspace, &active_cache);
                    Ok(Some(provider))
                })
                .await
                {
                    Ok(Ok(None)) => {
                        // Another engine on this machine is fetching into the
                        // shared cache. Not a failure, so the attempt counter
                        // and the degraded notification both stay put.
                        peer_wait = true;
                        log!(
                            @Memory,
                            "Another engine is downloading the embedding model into the shared \
                             cache; checking again in {}s",
                            PEER_DOWNLOAD_POLL_SECS
                        );
                    }
                    Ok(Ok(Some(provider))) => {
                        // The model loads fine but may not FIT: an existing
                        // workspace's vector column keeps the width it was
                        // created with. Refuse before installing, or every
                        // insert and every re-embed UPDATE fails on a
                        // dimension mismatch with nothing said out loud.
                        if let Some(message) = dimension_mismatch(&engine, &provider).await {
                            log!(
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
                        // `install` sets the slot to Ready; broadcast that.
                        engine.embedder().install(provider);
                        broadcast_status(&engine);
                        log!(
                            "[Memory] Embedding model loaded (attempt #{}) — memory features active",
                            attempt
                        );
                        // Construction skipped the startup re-embed sweep (the
                        // slot booted empty); run it now that embeds can succeed.
                        // This only re-embeds EXISTING rows carrying a stale model
                        // id — items dropped during the empty-slot window (no row
                        // ever inserted, see `index_memory_inner_impl`) are
                        // recovered by a manual memory rebuild, not here. See
                        // docs/known-gaps.md.
                        if let Some(index) = engine.memory_index().clone() {
                            let embedder = engine.embedder().clone();
                            if let Err(e) =
                                crate::memory::reembed::reembed_stale(&index, embedder).await
                            {
                                log!(@Memory, "Re-embed task failed: {}", e);
                            }
                        }
                        // Only announce recovery if the user was previously told
                        // memory was degraded — a normal boot is silent.
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
                        log!(
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
                        // Non-fetch failure (corrupt cached model, bad config):
                        // retrying can't fix it — stop and say so loudly. The
                        // workspace stays usable; only memory is disabled.
                        log!(
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
                        log!(
                            "[Memory] Embedding model load task panicked: {} — retrying",
                            join_err
                        );
                        // Say so, or the UI sits on the last download frame
                        // this attempt happened to emit for the whole backoff.
                        publish(&engine, EmbeddingModelLoadState::Waiting { attempt });
                    }
                }
            }
        });
    }
}

/// Notify via the engine's one notification chokepoint
/// (`LucidosEngine::create_notification` — DB row + SSE + the OS push /
/// native-banner fan-out). Best-effort — a failed emit only loses the
/// notification, never the load loop.
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
        log!("[Memory] Failed to emit embedder notification: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The load must try IMMEDIATELY (no sleep before the first attempt) so a
    /// warm cache comes online within seconds, then back off on failures and
    /// clamp to the last schedule entry — the invariant that keeps boot from
    /// paying the model download while still not hammering an offline HF.
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
        // Past the schedule, repeat the last (10 min) forever.
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

    /// Waiting on a peer is not a failed attempt. Since the model cache is
    /// shared by every workspace on the machine, a parallel cold start has one
    /// winner and the rest find the lock held: if that advanced the schedule,
    /// each loser would march to `NOTIFY_AFTER_ATTEMPTS` and announce a
    /// degradation while the download it is waiting for proceeds normally.
    #[test]
    fn a_peer_wait_polls_briefly_and_does_not_advance_the_backoff() {
        assert_eq!(
            delay_before_pass(0, true),
            Some(Duration::from_secs(PEER_DOWNLOAD_POLL_SECS)),
            "a peer wait polls, even before any real attempt has completed"
        );
        // However many peer waits happen, the schedule stays where the real
        // attempts left it, because the caller never advances the counter.
        assert_eq!(
            delay_before_pass(2, true),
            Some(Duration::from_secs(PEER_DOWNLOAD_POLL_SECS))
        );
        assert!(
            PEER_DOWNLOAD_POLL_SECS < RETRY_DELAYS_SECS[0],
            "a peer wait must be shorter than the first failure backoff, or the \
             loser lags the winner by minutes for no reason"
        );
        // Without a peer, the failure schedule is untouched.
        assert_eq!(delay_before_pass(0, false), None);
        assert_eq!(delay_before_pass(2, false), delay_before_attempt(2));
    }
}
