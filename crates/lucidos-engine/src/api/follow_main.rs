//! The follow task's one route: a nudge to ask `origin` now (ADR 0589).

use super::*;

/// POST /api/v1/follow-main/nudge: a paired gateway landed on `origin`, so the
/// follow task asks at once. Its own gateway sends it. The nudge moves nothing
/// itself: the next round fetches, fast-forwards and builds as usual.
pub(super) async fn nudge(State(state): State<AppState>) -> StatusCode {
    state.engine.nudge_follow_main();
    StatusCode::ACCEPTED
}

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/follow-main/nudge", post(nudge))
}
