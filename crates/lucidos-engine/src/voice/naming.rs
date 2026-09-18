//! Giving the thread a call runs on a name.
//!
//! A thread is named from its conversation, and a call's conversation is
//! spoken. The chat titler reads one message, which is the right unit for
//! something typed and the wrong one for speech: "Yeah, please check" is a
//! whole request and none of its subject. So a call is named from its
//! exchange, by `LucidosEngine::spawn_call_title_generation`.
//!
//! **A seam, so the call loop stays drivable without an engine.** `call.rs`
//! talks to a [`ThreadNamer`] the way it talks to a `TurnStarter` and a
//! `DecisionResolver`. The tests supply one that records.
//!
//! **Asking twice is ordinary.** The call loop asks on the first caller
//! utterance that followed a reply, and `api::voice` asks again once the call
//! is over. Whether either produces a name is settled on the far side, by
//! `LucidosEngine::spawn_call_title_generation` and its naming slot.

use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use crate::engine::LucidosEngine;

/// What a call does about its thread's name.
#[async_trait]
pub trait ThreadNamer: Send + Sync {
    /// This call has an exchange in it, so it can be named.
    ///
    /// **Returns at once, whatever it decides.** The caller is on the audio
    /// path, and naming reads the thread and calls a model. Whether the thread
    /// already has a name is decided on the other side of the spawn, so even
    /// that read is off this path.
    ///
    /// Asked twice per call, deliberately: the loop asks and the call's end
    /// asks, and neither knows what the other did.
    async fn name_this_call(&self, thread_id: Uuid);
}

/// The shipping namer: the ordinary title path, run off the audio path.
pub struct CallNamer {
    engine: Arc<LucidosEngine>,
}

impl CallNamer {
    pub fn new(engine: Arc<LucidosEngine>) -> Self {
        Self { engine }
    }
}

#[async_trait]
impl ThreadNamer for CallNamer {
    async fn name_this_call(&self, thread_id: Uuid) {
        let engine = Arc::clone(&self.engine);
        tokio::spawn(async move {
            engine.spawn_call_title_generation(thread_id).await;
        });
    }
}
