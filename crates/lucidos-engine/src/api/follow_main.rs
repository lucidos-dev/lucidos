//! The routes a gateway posts to its engines: a nudge to ask `origin` now
//! (ADR 0589), and a signal that the operators changed (ADR 0623).

use super::*;

/// POST /api/v1/follow-main/nudge: a paired gateway landed on `origin`, so the
/// follow task asks at once. Its own gateway sends it. The nudge moves nothing
/// itself: the next round fetches, fast-forwards and builds as usual.
pub(super) async fn nudge(State(state): State<AppState>) -> StatusCode {
    state.engine.nudge_follow_main();
    StatusCode::ACCEPTED
}

/// POST /api/v1/operators/changed: the gateway recorded an identity change,
/// so read the operators again (ADR 0623). Its own gateway sends it. Pages
/// hear `OperatorsChanged` once the new list differs.
pub(super) async fn operators_changed(State(state): State<AppState>) -> StatusCode {
    tokio::spawn(async move {
        if crate::gateway_operators::refresh().await {
            state
                .engine
                .event_bus
                .emit_or_log(
                    crate::engine::event_bus::BusEvent::System(
                        crate::engine::event_bus::SystemEvent::OperatorsChanged,
                    ),
                    "[Operators] OperatorsChanged",
                )
                .await;
        }
    });
    StatusCode::ACCEPTED
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/follow-main/nudge", post(nudge))
        .route("/operators/changed", post(operators_changed))
}
