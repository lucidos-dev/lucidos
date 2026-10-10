use super::*;

use crate::core::knowhow::{
    knowhow_not_found_body, load_one_knowhow_section, KnowhowStore, SYSTEM_KNOWHOW_PREFIX,
};
use crate::core::SystemKnowhowStore;
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::engine::thread_events::MessageOrigin;

/// Listing entry for knowhow surfaces — used by the file-preview "did you mean"
/// suggestion when a stale knowhow link 404s, and by any other UI that needs
/// the merged user + system knowhow set.
#[derive(Serialize)]
pub struct KnowhowEntry {
    /// The knowhow id. User-curated knowhow uses the path under `data/knowhow/`
    /// without `.md`; engine-shipped reference docs have the `system-knowhow/`
    /// prefix baked in. This is what `load_knowhow` accepts and what intent
    /// frontmatter (`knowhow: [...]`) references.
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Serialize)]
pub struct ListKnowhowResponse {
    pub knowhow: Vec<KnowhowEntry>,
}

/// List the knowhow docs. Returns the merged user knowhow (local > shared)
/// followed by engine-shipped system-knowhow with the `system-knowhow/`
/// prefix already applied to ids. Each group is alphabetical (inherited from
/// the loader); system entries follow user entries so callers (e.g. the
/// file-preview "did you mean" suggestion) can group them naturally.
///
/// Docs, not every file. A user root lists what
/// [`crate::core::KnowhowListDepth`] counts as a doc, so a doc's own
/// references are as absent here as from the Know-how routing list.
/// `/knowhow/read` still reads any of them by full id.
pub(super) async fn list_knowhow(State(state): State<AppState>) -> Json<ListKnowhowResponse> {
    let kh_dirs = state.engine.knowhow_dirs();
    let mut entries: Vec<KnowhowEntry> = KnowhowStore::load_merged_summaries(&kh_dirs)
        .into_iter()
        .map(|s| KnowhowEntry {
            id: s.id,
            name: s.name,
            description: s.description,
        })
        .collect();

    if let Some(sys_dir) = state.engine.system_knowhow_dir() {
        let sys = SystemKnowhowStore::load_summaries(sys_dir)
            .into_iter()
            .map(|s| KnowhowEntry {
                id: format!("{}{}", SYSTEM_KNOWHOW_PREFIX, s.id),
                name: s.name,
                description: s.description,
            });
        entries.extend(sys);
    }

    Json(ListKnowhowResponse { knowhow: entries })
}

/// Query for [`read_knowhow`].
#[derive(Deserialize)]
pub struct ReadKnowhowQuery {
    /// The knowhow id, exactly as it appears in the `/knowhow` listing — a
    /// user knowhow id (path under `data/knowhow/` without `.md`) or a
    /// `system-knowhow/`-prefixed engine-shipped doc.
    pub id: String,
}

/// Read a single knowhow doc's full content by id.
///
/// Returns the same `[KNOW-HOW: …]` / `[SYSTEM-KNOWHOW: …]` block the
/// `load_knowhow` LLM tool produces (via the shared
/// [`load_one_knowhow_section`]), so a coding-agent thread that lacks the
/// `load_knowhow` tool — e.g. an app coding-agent thread whose sparse-checkout
/// worktree can't see the engine's `system-knowhow/` on disk — can fetch the
/// identical guidance through `lucidos knowhow read <id>`. 404 with the shared
/// not-found sentinel when the id resolves to nothing.
pub(super) async fn read_knowhow(
    State(state): State<AppState>,
    Query(query): Query<ReadKnowhowQuery>,
    headers: axum::http::HeaderMap,
) -> Response {
    let kh_dirs = state.engine.knowhow_dirs();
    let sys_dir = state.engine.system_knowhow_dir();
    match load_one_knowhow_section(&kh_dirs, sys_dir, &query.id) {
        Some(section) => {
            record_knowhow_read(&state, &query.id, super::actor::user_actor(&headers, None)).await;
            section.into_response()
        }
        None => (StatusCode::NOT_FOUND, knowhow_not_found_body(&query.id)).into_response(),
    }
}

/// Held across the check and the emit in [`record_knowhow_read`]. Without it
/// two concurrent reads of one doc both see no row today, and both emit.
static KNOWHOW_READ_GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Record the read as use evidence for the workspace prompt footprint, which
/// judges in whole days. So one `KnowhowRead` per doc per day is enough, and
/// an app re-reading a doc in a loop adds no rows.
async fn record_knowhow_read(state: &AppState, id: &str, actor: Option<MessageOrigin>) {
    let _gate = KNOWHOW_READ_GATE.lock().await;
    let recorded_today = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM events WHERE event_type = 'KnowhowRead' \
         AND aggregate_id = $1 AND created >= now() - make_interval(days => 1))",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await;
    match recorded_today {
        Ok(true) => {}
        Ok(false) => {
            state
                .engine
                .event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::KnowhowRead {
                        knowhow_id: id.to_string(),
                        actor,
                    }),
                    "[Knowhow] KnowhowRead",
                )
                .await
        }
        Err(e) => crate::log!("[Knowhow] Cannot check for today's read of {}: {}", id, e),
    }
}

/// Route for the `/knowhow` surface.
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/knowhow", get(list_knowhow))
        .route("/knowhow/read", get(read_knowhow))
}
