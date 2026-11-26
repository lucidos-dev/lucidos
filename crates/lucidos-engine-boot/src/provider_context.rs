use lucidos_engine::llm::ProviderBuildContext;
use lucidos_engine::log_legacy;
use std::process::Command;

pub async fn build_provider_context(database_url: &str) -> ProviderBuildContext {
    let project_id = std::env::var("VERTEX_PROJECT_ID")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(lucidos_engine::llm::vertex::adc::project_from_files)
        .or_else(get_gcloud_project)
        .unwrap_or_default();

    let (initial_region, region_source) = match read_vertex_region_pref(database_url).await {
        Ok(Some(region)) => (region, "the vertex_region setting"),
        Ok(None) => vertex_region_from_env_or_default(),
        Err(e) => {
            log_legacy!(@Engine,
                "[Startup] WARNING: could not read the vertex_region setting ({}), \
                 falling back",
                e
            );
            vertex_region_from_env_or_default()
        }
    };
    let vertex_region = lucidos_engine::llm::vertex::location_handle(initial_region.clone());

    let model = std::env::var("LUCIDOS_MODEL").unwrap_or_else(|_| {
        lucidos_engine::core::prefs::CHAT_MODEL
            .default_text()
            .to_string()
    });

    log_legacy!(@Engine, "[Startup] Using default model: {}", model);

    let model_registry = lucidos_engine::llm::model_registry::empty();

    let model_is_mock = model == lucidos_engine::llm::MOCK_MODEL;

    if !model_is_mock {
        if project_id.is_empty() {
            log_legacy!(@Engine, "[Startup] Vertex AI not configured — Claude/Gemini models unavailable");
        } else {
            log_legacy!(@Engine,
                "[Startup] Vertex AI configured (project: {}, region: {} from {})",
                project_id,
                initial_region,
                region_source
            );
        }
    }

    let vertex_token_cache: Option<lucidos_engine::llm::vertex::TokenCache> = (!model_is_mock
        && !project_id.is_empty())
    .then(|| std::sync::Arc::new(std::sync::Mutex::new(None)));

    ProviderBuildContext {
        default_model: model,
        model_is_mock,
        vertex_project_id: project_id,
        vertex_location: vertex_region,
        vertex_token_cache,
        model_registry,
        not_served: Default::default(),
        claude_gateway_access: None,
        chatgpt_plan_access: None,
    }
}

const PG_UNDEFINED_TABLE: &str = "42P01";

async fn read_vertex_region_pref(
    database_url: &str,
) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(database_url)
        .await?;
    let stored = lucidos_engine::core::prefs::VERTEX_REGION
        .try_stored(&pool)
        .await;
    match stored {
        Ok(value) => Ok(value.filter(|s| !s.is_empty())),
        Err(sqlx::Error::Database(e)) if e.code().as_deref() == Some(PG_UNDEFINED_TABLE) => {
            Ok(None)
        }
        Err(e) => Err(e.into()),
    }
}

fn vertex_region_from_env_or_default() -> (String, &'static str) {
    match std::env::var("VERTEX_REGION")
        .ok()
        .filter(|s| !s.is_empty())
    {
        Some(region) => (region, "VERTEX_REGION"),
        None => (
            lucidos_engine::core::prefs::VERTEX_REGION
                .default_text()
                .to_string(),
            "the default",
        ),
    }
}

fn get_gcloud_project() -> Option<String> {
    Command::new("gcloud")
        .args(["config", "get-value", "project"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .filter(|s| !s.is_empty())
}
