use super::*;

pub(super) async fn nudge(State(state): State<AppState>) -> StatusCode {
    state.engine.nudge_follow_main();
    StatusCode::ACCEPTED
}

pub(super) async fn users_changed(State(state): State<AppState>) -> StatusCode {
    tokio::spawn(async move {
        announce_users_if_changed(&state.engine.event_bus, crate::gateway_users::refresh()).await;
    });
    StatusCode::ACCEPTED
}

/// INVARIANT: Announce `UsersChanged` once `changed` says a read brought a different
/// list. Every reader that resolved a device against the earlier list hears
/// it: a page, and a gateway's row stream.
///
/// The engine's boot owes this for a gateway that answered after serving
/// began (`gateway_users::late_first_answer`). Readers saw a team as one user
/// until then, and nothing else would tell them.
pub async fn announce_users_if_changed(
    bus: &crate::engine::event_bus::EventBus,
    changed: impl std::future::Future<Output = bool>,
) {
    if changed.await {
        bus.emit_or_log(
            crate::engine::event_bus::BusEvent::System(
                crate::engine::event_bus::SystemEvent::UsersChanged,
            ),
            "[Users] UsersChanged",
        )
        .await;
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(super) enum PairingRequestKind {
    Device,
    Gateway,
}

#[derive(Debug, Deserialize)]
pub(super) struct RaisedPairingRequest {
    kind: PairingRequestKind,
    /// INVARIANT: The redeemer's own name for itself. Its claim, so a hint only.
    label: String,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct PairingRequestsChangedBody {
    #[serde(default)]
    raised: Option<RaisedPairingRequest>,
}

/// INVARIANT: The notification a waiting pairing raises, title then body.
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
        crate::engine::permission::one_line(&raised.label),
        crate::scheduler::notifications::SettingsPage::ACCESS.link(),
    );
    (title.to_string(), message)
}

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

    #[tokio::test]
    async fn a_changed_user_list_is_announced_and_an_unchanged_one_is_not() {
        use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
        let (pool, db) = crate::test_support::setup_test_db().await;
        let (bus, _parent_rx) = EventBus::new(pool.clone());
        let mut events = bus.subscribe();

        announce_users_if_changed(&bus, async { false }).await;
        assert!(events.try_recv().is_err(), "nothing changed");

        announce_users_if_changed(&bus, async { true }).await;
        let heard = events.try_recv().expect("the change is announced");
        assert!(matches!(
            heard.typed,
            BusEvent::System(SystemEvent::UsersChanged)
        ));

        pool.close().await;
        crate::test_support::teardown_test_db(&db).await;
    }

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
