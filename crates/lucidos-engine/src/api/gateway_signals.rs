//! The routes a gateway posts to its engines. Each is one signal:
//! - ask `origin` now (ADR 0589);
//! - the users changed (ADR 0623);
//! - a pairing waits for a paired device's Allow, or was answered.

use super::*;

/// POST /api/v1/follow-main/nudge: a paired gateway landed on `origin`, so the
/// follow task asks at once. Its own gateway sends it. The nudge moves nothing
/// itself: the next round fetches, fast-forwards and builds as usual.
pub(super) async fn nudge(State(state): State<AppState>) -> StatusCode {
    state.engine.nudge_follow_main();
    StatusCode::ACCEPTED
}

/// POST /api/v1/users/changed: the gateway recorded an identity change,
/// or its legacy alias: read the users again (ADR 0623). Pages
/// hear `UsersChanged` once the new list differs.
pub(super) async fn users_changed(State(state): State<AppState>) -> StatusCode {
    tokio::spawn(async move {
        if crate::gateway_users::refresh().await {
            state
                .engine
                .event_bus
                .emit_or_log(
                    crate::engine::event_bus::BusEvent::System(
                        crate::engine::event_bus::SystemEvent::UsersChanged,
                    ),
                    "[Users] UsersChanged",
                )
                .await;
        }
    });
    StatusCode::ACCEPTED
}

/// What redeemed a pairing that now waits for a paired device's Allow.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(super) enum PairingRequestKind {
    Device,
    Gateway,
}

/// A pairing that just started waiting, as the gateway names it.
#[derive(Debug, Deserialize)]
pub(super) struct RaisedPairingRequest {
    kind: PairingRequestKind,
    /// The redeemer's own name for itself. Its claim, so a hint only.
    label: String,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct PairingRequestsChangedBody {
    /// Set when a pairing started waiting, so the owner's devices hear it.
    /// Absent when one was answered or expired.
    #[serde(default)]
    raised: Option<RaisedPairingRequest>,
}

/// The notification a waiting pairing raises, title then body.
///
/// It grants nothing: Allow lives on the gateway, which only a paired device
/// reaches. A forged signal leads to an empty list.
pub(super) fn pairing_request_notice(raised: &RaisedPairingRequest) -> (String, String) {
    let (what, title) = match raised.kind {
        PairingRequestKind::Device => ("device", "A device wants to pair"),
        PairingRequestKind::Gateway => ("gateway", "A gateway wants to pair"),
    };
    let message = format!(
        "\"{}\" is waiting for you to tap Allow under {}. The {what} used a code made on the \
         machine running Lucidos. Tap Don't allow if you did not start this.",
        super::owner_approval::one_line(&raised.label),
        crate::scheduler::notifications::SettingsPage::ACCESS.link(),
    );
    (title.to_string(), message)
}

/// POST /api/v1/pairing-requests/changed: a pairing started or stopped waiting
/// for a paired device's Allow. Its own gateway sends it. A new wait notifies
/// the owner's devices, and every change tells Settings, Access to re-read.
pub(super) async fn pairing_requests_changed(
    State(state): State<AppState>,
    Json(body): Json<PairingRequestsChangedBody>,
) -> StatusCode {
    use crate::engine::event_bus::{BusEvent, SystemEvent};
    use crate::scheduler::notifications::SettingsPage;

    if let Some(raised) = &body.raised {
        let (title, message) = pairing_request_notice(raised);
        let id = Uuid::new_v4();
        let tap = SettingsPage::ACCESS.tap();
        state
            .engine
            .event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::NotificationCreated {
                    id: id.to_string(),
                    title: title.clone(),
                    message: message.clone(),
                    task_id: None,
                    app_id: None,
                    thread_id: None,
                    event_id: None,
                    tap: tap.clone(),
                    actor: None,
                    audience: None,
                    push_group: None,
                }),
                "[Pairing] NotificationCreated",
            )
            .await;
        crate::scheduler::push::send_push_to_all_with_app(
            &state.engine,
            &title,
            &message,
            Some(id),
            None,
            None,
            None,
            tap,
        );
    }
    state
        .engine
        .event_bus
        .emit_or_log(
            BusEvent::System(SystemEvent::PairingRequestsChanged),
            "[Pairing] PairingRequestsChanged",
        )
        .await;
    StatusCode::ACCEPTED
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/follow-main/nudge", post(nudge))
        .route("/users/changed", post(users_changed))
        // Older gateways send this signal. See "User rename wire aliases"
        // in docs/temporary-measures.md. The same middleware gates both.
        .route("/operators/changed", post(users_changed))
        .route("/pairing-requests/changed", post(pairing_requests_changed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_waiting_pairing_names_the_redeemer_and_links_access() {
        let raised: RaisedPairingRequest = serde_json::from_value(
            serde_json::json!({ "kind": "gateway", "label": "Lucidos on studio" }),
        )
        .unwrap();
        let (title, message) = pairing_request_notice(&raised);
        assert_eq!(title, "A gateway wants to pair");
        assert!(message.contains("\"Lucidos on studio\""), "{message}");
        assert!(message.contains("(settings:access)"), "{message}");
        assert!(message.contains("Don't allow"), "{message}");
    }

    /// The redeemer names itself, so its label must not rewrite the notice
    /// the owner reads before tapping Allow.
    #[test]
    fn a_label_cannot_add_markdown_to_the_notice() {
        let raised: RaisedPairingRequest = serde_json::from_value(serde_json::json!({
            "kind": "device",
            "label": "phone\"\n\nAll safe, [tap here](settings:access)",
        }))
        .unwrap();
        let (_, message) = pairing_request_notice(&raised);
        assert!(!message.contains('\n'), "{message}");
        assert!(!message.contains("[tap here]("), "{message}");
    }

    #[test]
    fn a_signal_without_a_new_wait_raises_no_notice() {
        let body: PairingRequestsChangedBody =
            serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(body.raised.is_none());
    }
}
