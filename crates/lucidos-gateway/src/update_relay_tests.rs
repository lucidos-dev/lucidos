use super::*;

fn beat(version: &str, blocker: Option<&str>) -> Heartbeat {
    Heartbeat {
        version: version.to_string(),
        blocker: blocker.map(str::to_string),
        progress: None,
    }
}

fn beat_with(id: &str, frame: Value) -> Heartbeat {
    Heartbeat {
        version: "1.0.0".to_string(),
        blocker: None,
        progress: Some(RelayedProgress {
            request: id.to_string(),
            frame,
        }),
    }
}

/// A relay with an attached, unblocked client on 1.0.0.
fn attached(now: Instant) -> UpdateRelay {
    let relay = UpdateRelay::default();
    assert_eq!(relay.heartbeat(beat("1.0.0", None), now), None);
    relay
}

#[test]
fn a_fresh_gateway_reports_no_client_and_no_relay() {
    // An old client sends no heartbeat, so this is what it looks like.
    let relay = UpdateRelay::default();
    assert_eq!(
        relay.snapshot(Instant::now()),
        json!({ "client": null, "request": null })
    );
}

#[test]
fn a_request_with_no_client_is_refused() {
    let relay = UpdateRelay::default();
    assert_eq!(
        relay.request(Some("1.1.0"), Instant::now()),
        Err(Refusal::NoClient)
    );
}

#[test]
fn a_client_that_stopped_beating_is_not_attached() {
    let t0 = Instant::now();
    let relay = attached(t0);
    let later = t0 + ATTACHED_WITHIN;
    assert_eq!(relay.snapshot(later)["client"], Value::Null);
    assert_eq!(relay.request(Some("1.1.0"), later), Err(Refusal::NoClient));
}

#[test]
fn a_blocked_client_refuses_with_its_own_reason() {
    let now = Instant::now();
    let relay = UpdateRelay::default();
    relay.heartbeat(beat("1.0.0", Some("needs an admin password")), now);
    assert_eq!(
        relay.request(Some("1.1.0"), now),
        Err(Refusal::Blocked("needs an admin password".to_string()))
    );
    assert_eq!(
        relay.snapshot(now)["client"]["blocker"],
        "needs an admin password"
    );
}

#[test]
fn a_request_needs_a_release_newer_than_the_client() {
    let now = Instant::now();
    let relay = attached(now);
    for latest in [None, Some("1.0.0"), Some("0.9.9")] {
        assert_eq!(
            relay.request(latest, now),
            Err(Refusal::NoNewerRelease),
            "{latest:?}"
        );
    }
}

#[test]
fn a_request_is_handed_to_exactly_one_heartbeat() {
    let now = Instant::now();
    let relay = attached(now);
    let ticket = relay.request(Some("1.1.0"), now).unwrap();
    assert_eq!(ticket.version, "1.1.0");
    assert_eq!(relay.snapshot(now)["request"]["state"], "requested");

    assert_eq!(
        relay.heartbeat(beat("1.0.0", None), now),
        Some(ticket.clone())
    );
    assert_eq!(relay.heartbeat(beat("1.0.0", None), now), None);
    assert_eq!(relay.snapshot(now)["request"]["state"], "running");
}

#[test]
fn an_unclaimed_request_expires_and_is_never_handed_out() {
    let t0 = Instant::now();
    let relay = attached(t0);
    relay.request(Some("1.1.0"), t0).unwrap();
    let later = t0 + REQUEST_EXPIRES_AFTER;
    assert_eq!(relay.heartbeat(beat("1.0.0", None), later), None);
    assert_eq!(relay.snapshot(later)["request"], Value::Null);
}

#[test]
fn a_second_request_is_refused_while_one_is_pending_or_running() {
    let now = Instant::now();
    let relay = attached(now);
    relay.request(Some("1.1.0"), now).unwrap();
    assert_eq!(
        relay.request(Some("1.1.0"), now),
        Err(Refusal::InFlight),
        "pending"
    );
    relay.heartbeat(beat("1.0.0", None), now);
    assert_eq!(
        relay.request(Some("1.1.0"), now),
        Err(Refusal::InFlight),
        "running"
    );
}

#[test]
fn progress_reaches_the_snapshot_unchanged() {
    let now = Instant::now();
    let relay = attached(now);
    let ticket = relay.request(Some("1.1.0"), now).unwrap();
    relay.heartbeat(beat("1.0.0", None), now);
    let frame = json!({ "version": "1.1.0", "phase": "downloading", "downloaded": 5, "total": 10 });
    relay.heartbeat(beat_with(&ticket.id, frame.clone()), now);
    let snap = relay.snapshot(now);
    assert_eq!(snap["request"]["id"], ticket.id.as_str());
    assert_eq!(snap["request"]["state"], "running");
    assert_eq!(snap["request"]["progress"], frame);
}

#[test]
fn a_terminal_frame_ends_the_run_and_frees_the_slot() {
    // A bundle-swap failure carries the recovery message the requester must
    // read, so the frame survives whole.
    let now = Instant::now();
    let relay = attached(now);
    let ticket = relay.request(Some("1.1.0"), now).unwrap();
    relay.heartbeat(beat("1.0.0", None), now);
    let frame = json!({
        "version": "1.1.0",
        "phase": "bundle-swap-failed",
        "message": "Reinstall Lucidos from the .dmg to recover."
    });
    relay.heartbeat(beat_with(&ticket.id, frame.clone()), now);
    let snap = relay.snapshot(now);
    assert_eq!(snap["request"]["state"], "ended");
    assert_eq!(snap["request"]["progress"], frame);
    assert!(relay.request(Some("1.1.0"), now).is_ok());
}

#[test]
fn a_frame_for_another_request_is_ignored() {
    let now = Instant::now();
    let relay = attached(now);
    relay.request(Some("1.1.0"), now).unwrap();
    relay.heartbeat(beat("1.0.0", None), now);
    relay.heartbeat(
        beat_with("stale-1", json!({ "phase": "failed", "message": "old" })),
        now,
    );
    assert_eq!(relay.snapshot(now)["request"]["state"], "running");
}

#[test]
fn a_run_whose_client_stopped_beating_is_dropped() {
    let t0 = Instant::now();
    let relay = attached(t0);
    relay.request(Some("1.1.0"), t0).unwrap();
    relay.heartbeat(beat("1.0.0", None), t0);
    let later = t0 + ATTACHED_WITHIN;
    assert_eq!(relay.snapshot(later)["request"], Value::Null);
}

#[test]
fn ticket_ids_never_repeat() {
    let now = Instant::now();
    let relay = attached(now);
    let first = relay.request(Some("1.1.0"), now).unwrap();
    relay.heartbeat(beat("1.0.0", None), now);
    relay.heartbeat(beat_with(&first.id, json!({ "phase": "cancelled" })), now);
    let second = relay.request(Some("1.1.0"), now).unwrap();
    assert_ne!(first.id, second.id);
}

#[test]
fn the_relay_never_runs_a_process() {
    // ADR 0108: an installer the gateway spawns dies under `launchctl bootout`.
    // The install belongs to the desktop client, so nothing here may start one.
    let source = include_str!("update_relay.rs");
    for forbidden in ["Command::new", "std::process", "tokio::process"] {
        assert!(
            !source.contains(forbidden),
            "the relay must not run processes, found {forbidden}"
        );
    }
}

#[test]
fn a_claim_the_client_never_confirms_is_dropped() {
    // The reply carrying the ticket can be lost, say to a client read timeout.
    // The client then beats on with no progress, and the slot must free up.
    let t0 = Instant::now();
    let relay = attached(t0);
    relay.request(Some("1.1.0"), t0).unwrap();
    relay.heartbeat(beat("1.0.0", None), t0);
    let later = t0 + CLAIM_UNCONFIRMED_AFTER;
    relay.heartbeat(beat("1.0.0", None), later);
    assert_eq!(relay.snapshot(later)["request"], Value::Null);
    assert!(relay.request(Some("1.1.0"), later).is_ok());
}

#[test]
fn a_confirmed_run_outlives_the_claim_window() {
    // A long download keeps beating with progress, and must not be dropped.
    let t0 = Instant::now();
    let relay = attached(t0);
    let ticket = relay.request(Some("1.1.0"), t0).unwrap();
    relay.heartbeat(beat("1.0.0", None), t0);
    relay.heartbeat(beat_with(&ticket.id, json!({ "phase": "checking" })), t0);
    let later = t0 + CLAIM_UNCONFIRMED_AFTER * 4;
    relay.heartbeat(
        beat_with(&ticket.id, json!({ "phase": "downloading" })),
        later,
    );
    assert_eq!(relay.snapshot(later)["request"]["state"], "running");
}

// ── Refreshing the release check before refusing ────────────────────────────

mod refreshing {
    use super::*;
    use crate::release_check::tests::{deployment_in, http_ok, origin_serving, Origin};
    use crate::release_check::POLL_INTERVAL;

    const NEWER: &str = r#"{"version":"99.0.0"}"#;
    const NOTHING_NEWER: &str = "{}";

    async fn check_against(dir: &std::path::Path, body: &str) -> (ReleaseCheck, Origin) {
        let origin = origin_serving(http_ok(body), Duration::ZERO).await;
        let check =
            ReleaseCheck::with_origin(&deployment_in(dir, true), &origin.url, POLL_INTERVAL);
        (check, origin)
    }

    #[tokio::test]
    async fn a_release_the_gateway_has_not_polled_yet_is_found_and_queued() {
        let dir = tempfile::tempdir().unwrap();
        let (check, origin) = check_against(dir.path(), NEWER).await;
        let now = Instant::now();
        let relay = attached(now);
        assert_eq!(check.latest_version(), None);

        let ticket = relay.request_refreshing(&check, now).await.unwrap();

        assert_eq!(ticket.version, "99.0.0");
        assert_eq!(origin.requests(), 1);
    }

    #[tokio::test]
    async fn an_origin_naming_nothing_newer_still_refuses() {
        let dir = tempfile::tempdir().unwrap();
        let (check, origin) = check_against(dir.path(), NOTHING_NEWER).await;
        let now = Instant::now();
        let relay = attached(now);

        let refused = relay.request_refreshing(&check, now).await;

        assert_eq!(refused, Err(Refusal::NoNewerRelease));
        assert_eq!(origin.requests(), 1);
        assert_eq!(relay.snapshot(now)["request"], Value::Null);
    }

    #[tokio::test]
    async fn a_failed_poll_says_so_instead_of_claiming_nothing_is_published() {
        let dir = tempfile::tempdir().unwrap();
        let down =
            "HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
        let origin = origin_serving(down.to_string(), Duration::ZERO).await;
        let check =
            ReleaseCheck::with_origin(&deployment_in(dir.path(), true), &origin.url, POLL_INTERVAL);
        let now = Instant::now();
        let relay = attached(now);

        let refused = relay.request_refreshing(&check, now).await;

        assert!(
            matches!(refused, Err(Refusal::CheckFailed(_))),
            "{refused:?}"
        );
        assert!(refused.unwrap_err().message().contains("503"));
    }

    #[tokio::test]
    async fn a_release_already_known_is_queued_without_a_poll() {
        let dir = tempfile::tempdir().unwrap();
        let (check, origin) = check_against(dir.path(), NEWER).await;
        check.refresh(false).await;
        let now = Instant::now();
        let relay = attached(now);

        assert!(relay.request_refreshing(&check, now).await.is_ok());
        assert_eq!(origin.requests(), 1);
    }

    #[tokio::test]
    async fn only_a_missing_newer_release_reaches_the_origin() {
        let dir = tempfile::tempdir().unwrap();
        let (check, origin) = check_against(dir.path(), NEWER).await;
        let now = Instant::now();

        let unattached = UpdateRelay::default();
        assert_eq!(
            unattached.request_refreshing(&check, now).await,
            Err(Refusal::NoClient)
        );

        let blocked = UpdateRelay::default();
        blocked.heartbeat(beat("1.0.0", Some("needs an admin password")), now);
        assert_eq!(
            blocked.request_refreshing(&check, now).await,
            Err(Refusal::Blocked("needs an admin password".to_string()))
        );

        let busy = attached(now);
        busy.request(Some("1.1.0"), now).unwrap();
        assert_eq!(
            busy.request_refreshing(&check, now).await,
            Err(Refusal::InFlight)
        );

        assert_eq!(origin.requests(), 0);
    }
}
