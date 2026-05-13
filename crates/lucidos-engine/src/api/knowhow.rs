use super::*;

use crate::core::knowhow::{KnowhowStore, SYSTEM_KNOWHOW_PREFIX};
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
