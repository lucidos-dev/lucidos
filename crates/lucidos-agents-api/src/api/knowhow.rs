use super::*;

use crate::core::knowhow::{
    knowhow_is_oversized, knowhow_not_found_body, load_one_knowhow_section, KnowhowStore,
    SYSTEM_KNOWHOW_PREFIX,
};

const HEADER_KNOWHOW_OVERSIZED: &str = "lucidos-knowhow-oversized";
use crate::core::SystemKnowhowStore;
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::engine::thread_events::MessageOrigin;

#[derive(Serialize)]
pub struct KnowhowEntry {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Serialize)]
pub struct ListKnowhowResponse {
    pub knowhow: Vec<KnowhowEntry>,
}

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

#[derive(Deserialize)]
pub struct ReadKnowhowQuery {
    pub id: String,
}

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
            let oversized = knowhow_is_oversized(&section).then_some(section.len());
            let mut response = section.into_response();
            if let Some(bytes) = oversized {
                response.headers_mut().insert(
                    HEADER_KNOWHOW_OVERSIZED,
                    axum::http::HeaderValue::from(bytes),
                );
            }
            response
        }
        None => (StatusCode::NOT_FOUND, knowhow_not_found_body(&query.id)).into_response(),
    }
}

/// INVARIANT: Held across the check and the emit in [`record_knowhow_read`]. Without it
/// two concurrent reads of one doc both see no row today, and both emit.
static KNOWHOW_READ_GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
        Err(e) => crate::log_legacy!("[Knowhow] Cannot check for today's read of {}: {}", id, e),
    }
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/knowhow", get(list_knowhow))
        .route("/knowhow/read", get(read_knowhow))
}
