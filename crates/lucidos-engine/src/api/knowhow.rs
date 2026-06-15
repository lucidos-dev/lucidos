use super::*;

use crate::core::knowhow::{
    knowhow_not_found_body, load_one_knowhow_section, KnowhowStore, SYSTEM_KNOWHOW_PREFIX,
};
use crate::core::SystemKnowhowStore;

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

/// List all known knowhow ids. Returns the merged user knowhow (local >
/// shared) followed by engine-shipped system-knowhow with the
/// `system-knowhow/` prefix already applied to ids. Each group is
/// alphabetical (inherited from the loader); system entries follow user
/// entries so callers (e.g. the file-preview "did you mean" suggestion)
/// can group them naturally.
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
) -> Response {
    let kh_dirs = state.engine.knowhow_dirs();
    let sys_dir = state.engine.system_knowhow_dir();
    match load_one_knowhow_section(&kh_dirs, sys_dir, &query.id) {
        Some(section) => section.into_response(),
        None => (StatusCode::NOT_FOUND, knowhow_not_found_body(&query.id)).into_response(),
    }
}

/// Route for the `/knowhow` surface.
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/knowhow", get(list_knowhow))
        .route("/knowhow/read", get(read_knowhow))
}
