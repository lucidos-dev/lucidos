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

use super::provider::VoiceProvider;
use super::realtime::RealtimeProvider;
use crate::core::{
    preference_catalog, CredentialStore, PREF_MODEL_VOICE_TALKER, PREF_MODEL_VOICE_TRANSCRIBER,
    PREF_VOICE_TALKER_VOICE,
};
use crate::engine::{aux_purpose, ContextPurpose, LucidosEngine};
use crate::llm::{openai::codex_detect, resolve_openai_api_key};

/// The model this call speaks through.
///
/// `resolve_selection` reads an unset preference as an empty string, which the
/// extraction purposes take as "the extractor's own default". Voice has no such
/// reading: an empty model id names no socket to open. So the catalog default
/// is the second source here. That keeps the value Settings shows and the value
/// the call dials one string, not two.
async fn talker_model(pool: &PgPool) -> Result<String, String> {
    let selected = aux_purpose::resolve_selection(pool, ContextPurpose::Voice)
        .await
        .model;
    if !selected.trim().is_empty() {
        return Ok(selected);
    }
    match preference_catalog::lookup(PREF_MODEL_VOICE_TALKER) {
        Some(spec) if !spec.default.trim().is_empty() => Ok(spec.default.to_string()),
        _ => Err("model_voice_talker resolves to nothing".to_string()),
    }
}

/// A preference's value, or its catalog default when nothing is stored.
///
/// The pattern [`talker_model`] spells out above, for the keys that have no
/// second source to fall back to. A key missing from the catalog resolves to
/// the empty string, and every reader of one treats that as "use your own".
async fn or_catalog_default(pool: &PgPool, key: &'static str) -> String {
    if let Some(stored) = super::read_pref(pool, key).await {
        return stored;
    }
    preference_catalog::lookup(key)
        .map(|spec| spec.default.to_string())
        .unwrap_or_default()
}

/// The model this call transcribes the caller with.
///
/// Read here rather than through a [`ContextPurpose`]. That module pairs one
/// purpose with one budget and one capture, and this model makes no HTTP call
/// of its own: it is a field in the socket's opening frame. A purpose would add
/// a deadline nothing enforces and a wire enum variant nobody reads.
pub async fn transcriber_model(pool: &PgPool) -> String {
    or_catalog_default(pool, PREF_MODEL_VOICE_TRANSCRIBER).await
}

/// The voice this call speaks in. A provider's own name for one, not a model.
pub async fn talker_voice(pool: &PgPool) -> String {
    or_catalog_default(pool, PREF_VOICE_TALKER_VOICE).await
}

/// The talker to open this call on.
///
/// `Err` carries an engine-side sentence for the log, never one for a client:
/// naming a provider to a browser page is what the plan's decision 3 forbids.
pub async fn provider_for(engine: &LucidosEngine) -> Result<Arc<dyn VoiceProvider>, String> {
    let pool = engine.pool();
    let model = talker_model(pool).await?;

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
        let spec = preference_catalog::lookup(PREF_MODEL_VOICE_TALKER)
            .expect("the voice talker is agent-settable");
        assert!(
            !spec.default.trim().is_empty(),
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

        let model = talker_model(&pool)
            .await
            .expect("a fresh workspace can call");

        let default = preference_catalog::lookup(PREF_MODEL_VOICE_TALKER)
            .expect("catalog")
            .default;
        assert_eq!(model, default);
        assert!(!model.is_empty());
        teardown_test_db(&db).await;
    }

    /// The fallback is a second source, never an override. A user who pinned a
    /// realtime model must keep calling it.
    #[tokio::test]
    async fn a_stored_preference_wins_over_the_catalog_default() {
        let (pool, db) = setup_test_db().await;
        seed_preference(&pool, PREF_MODEL_VOICE_TALKER, "gpt-realtime-mini")
            .await
            .expect("seed");

        let model = talker_model(&pool).await.expect("a pinned model resolves");

        assert_eq!(model, "gpt-realtime-mini");
        teardown_test_db(&db).await;
    }

    /// Same promise as the talker's, for the two keys that arrived with the
    /// settings screen. A workspace that never opened it opens the call it
    /// always opened.
    #[tokio::test]
    async fn a_fresh_workspace_gets_the_transcriber_and_voice_it_always_had() {
        let (pool, db) = setup_test_db().await;

        assert_eq!(transcriber_model(&pool).await, "gpt-4o-mini-transcribe");
        assert_eq!(talker_voice(&pool).await, "marin");

        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_stored_transcriber_and_voice_win_over_their_defaults() {
        let (pool, db) = setup_test_db().await;
        seed_preference(&pool, PREF_MODEL_VOICE_TRANSCRIBER, "whisper-1")
            .await
            .expect("seed");
        seed_preference(&pool, PREF_VOICE_TALKER_VOICE, "cedar")
            .await
            .expect("seed");

        assert_eq!(transcriber_model(&pool).await, "whisper-1");
        assert_eq!(talker_voice(&pool).await, "cedar");

        teardown_test_db(&db).await;
    }

    /// Both keys are catalog rows, so both have a default to fall back on. An
    /// empty one would open a mute call, or a call with no voice.
    #[test]
    fn the_catalog_names_a_transcriber_and_a_voice() {
        for key in [PREF_MODEL_VOICE_TRANSCRIBER, PREF_VOICE_TALKER_VOICE] {
            let spec = preference_catalog::lookup(key).expect("an agent-settable voice key");
            assert!(!spec.default.trim().is_empty(), "{} has no default", key);
        }
    }
}
