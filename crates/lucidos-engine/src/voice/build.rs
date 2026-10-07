//! Which talker this workspace calls, resolved from what it has configured.
//!
//! One place decides, so the socket handler never reads a preference or a
//! credential itself. Everything it learns is `Ok(provider)` or a sentence
//! saying why there is none.
//!
//! Resolved per call rather than held from boot. A user who stores an OpenAI
//! key in Settings and then presses the microphone should be talking, not
//! restarting the engine.

use std::sync::Arc;

use sqlx::PgPool;

use super::live::LiveProvider;
use super::provider::VoiceProvider;
use super::realtime::RealtimeProvider;
use crate::core::{prefs, CredentialStore};
use crate::engine::LucidosEngine;
use crate::llm::{openai::codex_detect, resolve_openai_api_key};

/// The model this call speaks through: the stored one, else the catalog
/// default. Voice has no "use your own" reading of an unset model, because an
/// empty id names no socket to open. Settings shows the same string the call
/// dials.
async fn talker_model(pool: &PgPool) -> String {
    prefs::MODEL_VOICE_TALKER.read(pool).await
}

/// The model this call transcribes the caller with.
///
/// Read here rather than through a `ContextPurpose`. That module pairs one
/// purpose with one budget and one capture, and this model makes no HTTP call
/// of its own: it is a field in the socket's opening frame. A purpose would add
/// a deadline nothing enforces and a wire enum variant nobody reads.
pub async fn transcriber_model(pool: &PgPool) -> String {
    prefs::MODEL_VOICE_TRANSCRIBER.read(pool).await
}

/// The voice this call speaks in. A provider's own name for one, not a model.
pub async fn talker_voice(pool: &PgPool) -> String {
    prefs::VOICE_TALKER_VOICE.read(pool).await
}

/// Which protocol a model id speaks. The one place that decides.
///
/// A prefix rather than a list, because the family grows without us and an
/// unlisted `gpt-live-*` id belongs to the Live API by construction. Everything
/// else is Realtime, which is what an unknown id was before Live existed.
fn speaks_live(model: &str) -> bool {
    model.trim().starts_with("gpt-live")
}

/// The talker to open this call on.
///
/// `Err` carries an engine-side sentence for the log, never one for a client:
/// naming a provider to a browser page is what the plan's decision 3 forbids.
pub async fn provider_for(engine: &LucidosEngine) -> Result<Arc<dyn VoiceProvider>, String> {
    let pool = engine.pool();
    let model = talker_model(pool).await;

    // The switch is a veto over a provider that is otherwise configured. A user
    // who turned OpenAI off must not find voice still calling it.
    if !crate::llm::provider_build::read_provider_switches(pool)
        .await
        .openai
    {
        return Err("the OpenAI provider is switched off".to_string());
    }

    // The same three sources a chat call resolves on, in the same order. Voice
    // adds none of its own.
    let credential = match CredentialStore::get(pool, "openai").await {
        Ok(Some(cred)) => Some((cred.auth_type, cred.auth_value)),
        Ok(None) => None,
        Err(e) => {
            log!("[Voice] Could not read the OpenAI credential: {}", e);
            None
        }
    };
    let Some((api_key, source)) = resolve_openai_api_key(
        credential,
        std::env::var("OPENAI_API_KEY").ok(),
        codex_detect::load(),
    ) else {
        return Err("no OpenAI key is configured".to_string());
    };

    log!("[Voice] Calling {} with the key from {}", model, source);
    // The two protocols share a key and nothing else, so the id decides here
    // and nowhere above. Both answer the same seam, so no caller learns which.
    if speaks_live(&model) {
        return Ok(Arc::new(LiveProvider::new(api_key, model)));
    }
    Ok(Arc::new(RealtimeProvider::new(api_key, model)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{seed_preference, setup_test_db, teardown_test_db};

    /// The catalog default is the whole fallback, so an empty one puts the
    /// microphone straight back where this test found it.
    #[test]
    fn the_catalog_names_a_talker_to_fall_back_to() {
        assert!(
            !prefs::MODEL_VOICE_TALKER.default_text().trim().is_empty(),
            "an empty catalog default leaves a fresh workspace with a dead \
             voice button"
        );
    }

    /// The bug. A workspace that never wrote the preference resolved an empty
    /// model, and the socket handler answered every call with "No voice model
    /// is configured".
    #[tokio::test]
    async fn an_unset_preference_still_names_a_talker() {
        let (pool, db) = setup_test_db().await;

        let model = talker_model(&pool).await;

        assert_eq!(model, prefs::MODEL_VOICE_TALKER.default_text());
        assert!(!model.is_empty());
        teardown_test_db(&db).await;
    }

    /// The fallback is a second source, never an override. A user who pinned a
    /// realtime model must keep calling it.
    #[tokio::test]
    async fn a_stored_preference_wins_over_the_catalog_default() {
        let (pool, db) = setup_test_db().await;
        seed_preference(&pool, prefs::MODEL_VOICE_TALKER.key(), "gpt-realtime-mini")
            .await
            .expect("seed");

        let model = talker_model(&pool).await;

        assert_eq!(model, "gpt-realtime-mini");
        teardown_test_db(&db).await;
    }

    /// Same promise as the talker's, for the two keys that arrived with the
    /// settings screen. A workspace that never opened it opens the call it
    /// always opened.
    #[tokio::test]
    async fn a_fresh_workspace_gets_the_transcriber_and_voice_it_always_had() {
        let (pool, db) = setup_test_db().await;

        assert_eq!(
            transcriber_model(&pool).await,
            prefs::MODEL_VOICE_TRANSCRIBER.default_text()
        );
        assert_eq!(
            talker_voice(&pool).await,
            prefs::VOICE_TALKER_VOICE.default_text()
        );

        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_stored_transcriber_and_voice_win_over_their_defaults() {
        let (pool, db) = setup_test_db().await;
        seed_preference(&pool, prefs::MODEL_VOICE_TRANSCRIBER.key(), "whisper-1")
            .await
            .expect("seed");
        seed_preference(&pool, prefs::VOICE_TALKER_VOICE.key(), "cedar")
            .await
            .expect("seed");

        assert_eq!(transcriber_model(&pool).await, "whisper-1");
        assert_eq!(talker_voice(&pool).await, "cedar");

        teardown_test_db(&db).await;
    }

    /// Every id the talker picker offers, and the protocol it speaks. A
    /// Realtime id sent to the Live socket opens nothing, and the other way
    /// round is the same failure.
    #[test]
    fn each_talker_model_routes_to_the_protocol_it_speaks() {
        for realtime in [
            "gpt-realtime-2.1",
            "gpt-realtime-2.1-mini",
            "gpt-realtime-2",
            "gpt-realtime-1.5",
            "gpt-realtime-mini",
            "gpt-realtime",
        ] {
            assert!(!speaks_live(realtime), "{} was routed to Live", realtime);
        }
        for live in ["gpt-live-1", "gpt-live-1-mini", "gpt-live-2"] {
            assert!(speaks_live(live), "{} was routed to Realtime", live);
        }
    }

    /// An id nobody has heard of keeps the behaviour it had before Live
    /// existed. A model the agent pinned must still reach a socket.
    #[test]
    fn an_unknown_model_still_speaks_realtime() {
        for unknown in ["", "  ", "some-new-model", "whisper-1"] {
            assert!(!speaks_live(unknown), "{:?} was routed to Live", unknown);
        }
    }

    /// The catalog default is the id the picker leads with, so a fresh
    /// workspace dials a Realtime model rather than a per-minute one.
    #[test]
    fn a_fresh_workspace_dials_realtime_and_never_live() {
        assert!(!speaks_live(prefs::MODEL_VOICE_TALKER.default_text()));
    }

    /// Both keys are catalog rows, so both have a default to fall back on. An
    /// empty one would open a mute call, or a call with no voice.
    #[test]
    fn the_catalog_names_a_transcriber_and_a_voice() {
        for pref in [&prefs::MODEL_VOICE_TRANSCRIBER, &prefs::VOICE_TALKER_VOICE] {
            assert!(
                !pref.default_text().trim().is_empty(),
                "{} has no default",
                pref.key()
            );
        }
    }
}
