//! Background recovery for a degraded (empty) [`EmbedderSlot`] boot.
//!
//! When the first-run embedding-model download fails (offline packaged
//! install, HuggingFace blocked), engine construction boots with an empty
//! slot instead of dying (see `engine_impl/construction.rs`). This task
//! retries the model init with capped exponential backoff and installs the
//! provider into the live slot on success — memory features come back without
//! a restart, honoring engine statelessness (nothing here is persistent
//! state; a restart simply re-runs the same construction decision).

use std::sync::Arc;

use crate::engine::LucidosEngine;
use crate::memory::fastembed::{is_model_fetch_failure, FastEmbedProvider};

/// Backoff schedule between attempts; the last entry repeats forever. Starts
/// quick (a router blip on first open should heal fast) and settles at 10
/// minutes so an offline machine isn't hammering HuggingFace.
const RETRY_DELAYS_SECS: &[u64] = &[30, 60, 120, 300, 600];

/// Attempt number after which the user is told (once) that memory is degraded
/// — early enough to explain missing recall, late enough to skip transient
/// blips that heal on the first retry.
const NOTIFY_AFTER_ATTEMPTS: u32 = 3;

impl LucidosEngine {
    /// Spawn the background embedder retry when the boot left the slot empty.
    /// No-op on a normal (ready) boot. Called from `main.rs` once the engine
    /// is assembled (the task needs the event bus for notifications).
    pub fn spawn_embedder_retry_if_degraded(self: &Arc<Self>) {
        if self.embedder().is_ready() {
            return;
        }
        let engine = Arc::clone(self);
        tokio::spawn(async move {
            let mut attempt: u32 = 0;
            loop {
                let delay = RETRY_DELAYS_SECS
                    .get(attempt as usize)
                    .copied()
                    .unwrap_or(*RETRY_DELAYS_SECS.last().expect("non-empty schedule"));
                tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                if engine.is_shutting_down() {
                    return;
                }
                attempt += 1;
                match tokio::task::spawn_blocking(FastEmbedProvider::new).await {
                    Ok(Ok(provider)) => {
                        engine.embedder().install(provider);
                        log!(
                            "[Memory] Embedding model downloaded on retry #{} — memory features active",
                            attempt
                        );
                        // The startup re-embed sweep was skipped on the degraded
                        // boot; run it now that embeds can succeed.
                        if let Some(index) = engine.memory_index().clone() {
                            let embedder = engine.embedder().clone();
                            if let Err(e) =
                                crate::memory::reembed::reembed_stale(&index, embedder).await
                            {
                                log!(@Memory, "Re-embed task failed: {}", e);
                            }
                        }
                        notify(
                            &engine,
                            "Memory is ready",
                            "The embedding model finished downloading — memory search, \
                             extraction, and semantic thread search are now active.",
                        )
                        .await;
                        return;
                    }
                    Ok(Err(e)) if is_model_fetch_failure(e.as_ref()) => {
                        log!(
                            "[Memory] Embedding model retry #{} failed (fetch): {} — next attempt in the background",
                            attempt,
                            e
                        );
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
                        }
                    }
                    Ok(Err(e)) => {
                        // Non-fetch failure (corrupt cached model, bad config):
                        // retrying can't fix it — stop and say so loudly.
                        log!(
                            "[Memory] Embedding model init failed with a NON-network error: {} — \
                             giving up (fix the model cache / config and restart)",
                            e
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
                        log!(
                            "[Memory] Embedding model retry task panicked: {} — retrying",
                            join_err
                        );
                    }
                }
            }
        });
    }
}

/// Notify via the engine's one notification chokepoint
/// (`LucidosEngine::create_notification` — DB row + SSE + the OS push /
/// native-banner fan-out). Best-effort — a failed emit only loses the
/// notification, never the retry loop.
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
