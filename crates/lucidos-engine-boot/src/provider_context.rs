use lucidos_engine::llm::ProviderBuildContext;
use lucidos_engine::log_legacy;
use std::process::Command;

/// Assemble the provider inputs before engine construction applies migrations.
pub async fn build_provider_context(database_url: &str) -> ProviderBuildContext {
    // Resolve the Vertex project WITHOUT requiring the `gcloud` binary, so a
    // packaged build works from the user's existing ADC. The subprocess call is
    // the last resort.
    let project_id = std::env::var("VERTEX_PROJECT_ID")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(lucidos_engine::llm::vertex::adc::project_from_files)
        .or_else(get_gcloud_project)
        .unwrap_or_default();

    // The log line below names which input won. Nothing else tells a user who
    // set the region in Settings why requests still go somewhere else. An
    // exported VERTEX_REGION only counts if this process inherited it.
    let (initial_region, region_source) = match read_vertex_region_pref(database_url).await {
        Ok(Some(region)) => (region, "the vertex_region setting"),
        Ok(None) => vertex_region_from_env_or_default(),
        Err(e) => {
            // Falls back exactly as an absent setting does, but says so. A
            // silent fallback here reads as "you never set a region".
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

    // Shared model→provider routing map. The engine loads it after migrations
    // and hands it to the reload subscriber, which hot-swaps it on Model* events.
    let model_registry = lucidos_engine::llm::model_registry::empty();

    // The engine builds the provider from these inputs once the secret store
    // key loads, since a stored provider key is sealed (ADR 0509). The runtime
    // credential subscriber rebuilds from the same inputs.
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

    // Created up front so the SAME handle reaches the boot provider, the engine
    // and the credential subscriber's rebuilds, reusing warm access tokens.
    // `Some` exactly when a real Vertex build is possible.
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
        // The engine seeds it from the events once its migrations have run.
        not_served: Default::default(),
        // No event bus yet, so no gateway access. The engine rebuilds
        // with one once it has a bus.
        claude_gateway_access: None,
        chatgpt_plan_access: None,
    }
}

/// Postgres `undefined_table`. This read runs before `LucidosEngine::new` has
/// applied the migrations, so a brand-new workspace has no `preferences` table
/// on its first boot.
const PG_UNDEFINED_TABLE: &str = "42P01";

/// The stored `vertex_region`, or `None` when no row holds one.
///
/// `Err` means the read could not run, which is NOT the same answer as `None`.
/// A cold Postgres can time out the 5s acquire, and the caller then falls back
/// to a region the user never chose. It says which happened rather than
/// reporting the setting as absent.
///
/// A missing table is the one failure that IS `None`: no table means no stored
/// setting, and warning about it would fire on every first boot.
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

/// The Vertex region to use when no stored setting supplies one, and the name
/// of the input that supplied it.
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
