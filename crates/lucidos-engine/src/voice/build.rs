use std::sync::Arc;

use sqlx::PgPool;

use super::live::LiveProvider;
use super::provider::VoiceProvider;
use super::realtime::RealtimeProvider;
use crate::core::prefs;
use crate::engine::LucidosEngine;
use crate::llm::openai::openai_api_key;

async fn talker_model(pool: &PgPool) -> String {
    prefs::MODEL_VOICE_TALKER.read(pool).await
}

pub async fn transcriber_model(pool: &PgPool) -> String {
    prefs::MODEL_VOICE_TRANSCRIBER.read(pool).await
}

pub async fn talker_voice(pool: &PgPool) -> String {
    prefs::VOICE_TALKER_VOICE.read(pool).await
}

fn speaks_live(model: &str) -> bool {
    model.trim().starts_with("gpt-live")
}

/// INVARIANT: The talker to open this call on.
///
/// `Err` carries an engine-side sentence for the log, never one for a client:
/// naming a provider to a browser page is what the plan's decision 3 forbids.
pub async fn provider_for(engine: &LucidosEngine) -> Result<Arc<dyn VoiceProvider>, String> {
    let pool = engine.pool();
    let model = talker_model(pool).await;

    if !crate::llm::provider_preferences::read_provider_switches(pool)
        .await
        .openai
    {
        return Err("the OpenAI provider is switched off".to_string());
    }

    let Some((api_key, source)) = openai_api_key(pool).await else {
        return Err("no OpenAI key is configured".to_string());
    };

    log_legacy!("[Voice] Calling {} with the key from {}", model, source);
    if speaks_live(&model) {
        return Ok(Arc::new(LiveProvider::new(api_key, model)));
    }
    Ok(Arc::new(RealtimeProvider::new(api_key, model)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{seed_preference, setup_test_db, teardown_test_db};

    #[test]
    fn the_catalog_names_a_talker_to_fall_back_to() {
        assert!(
            !prefs::MODEL_VOICE_TALKER.default_text().trim().is_empty(),
            "an empty catalog default leaves a fresh workspace with a dead \
             voice button"
        );
    }

    #[tokio::test]
    async fn an_unset_preference_still_names_a_talker() {
        let (pool, db) = setup_test_db().await;

        let model = talker_model(&pool).await;

        assert_eq!(model, prefs::MODEL_VOICE_TALKER.default_text());
        assert!(!model.is_empty());
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_stored_preference_wins_over_the_catalog_default() {
        let (pool, db) = setup_test_db().await;
        seed_preference(
            &pool,
            prefs::MODEL_VOICE_TALKER.key(),
            "gpt-realtime-2.1-mini",
        )
        .await
        .expect("seed");

        let model = talker_model(&pool).await;

        assert_eq!(model, "gpt-realtime-2.1-mini");
        teardown_test_db(&db).await;
    }

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
        seed_preference(
            &pool,
            prefs::MODEL_VOICE_TRANSCRIBER.key(),
            "gpt-live-transcribe",
        )
        .await
        .expect("seed");
        seed_preference(&pool, prefs::VOICE_TALKER_VOICE.key(), "cedar")
            .await
            .expect("seed");

        assert_eq!(transcriber_model(&pool).await, "gpt-live-transcribe");
        assert_eq!(talker_voice(&pool).await, "cedar");

        teardown_test_db(&db).await;
    }

    #[test]
    fn each_talker_model_routes_to_the_protocol_it_speaks() {
        for realtime in [
            "gpt-realtime-2.1",
            "gpt-realtime-2.1-mini",
            "gpt-realtime-2",
            "gpt-realtime-1.5",
        ] {
            assert!(!speaks_live(realtime), "{} was routed to Live", realtime);
        }
        for live in ["gpt-live-1", "gpt-live-1-mini", "gpt-live-2"] {
            assert!(speaks_live(live), "{} was routed to Realtime", live);
        }
    }

    #[test]
    fn an_unknown_model_still_speaks_realtime() {
        for unknown in ["", "  ", "some-new-model", "gpt-transcribe"] {
            assert!(!speaks_live(unknown), "{:?} was routed to Live", unknown);
        }
    }

    #[test]
    fn a_fresh_workspace_dials_realtime_and_never_live() {
        assert!(!speaks_live(prefs::MODEL_VOICE_TALKER.default_text()));
    }

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
