use super::*;

use crate::scheduler::notifications::{NavigateTarget, NavigateUi, Tap};

fn nav_thread(id: &str, event_id: Option<&str>) -> Tap {
    Tap::Navigate {
        to: NavigateUi {
            target: NavigateTarget::Thread,
            id: Some(id.to_string()),
            event_id: event_id.map(str::to_string),
            ..Default::default()
        },
    }
}

fn nav_app(app_id: &str) -> Tap {
    Tap::Navigate {
        to: NavigateUi {
            target: NavigateTarget::App,
            app_id: Some(app_id.to_string()),
            ..Default::default()
        },
    }
}

#[test]
fn declarative_envelope_has_web_push_magic_and_notification_object() {
    // Top-level shape must opt in to declarative parsing per the Push API
    // spec — Safari 18.5+ keys off `web_push: 8030` to read the declarative
    // `notification.navigate` tap target. The iOS SW `push` handler may still
    // run to render the visible banner, so the declarative URL has to be right.
    let payload = build_push_payload("Hi", "There", None, None, None, None, &Tap::Modal, None);
    assert_eq!(payload["web_push"], 8030);
    assert!(payload["notification"].is_object());
    assert!(
        payload.get("title").is_none(),
        "title must be inside notification, not top-level"
    );
    assert!(
        payload.get("body").is_none(),
        "body must be inside notification, not top-level"
    );
    assert!(
        payload.get("data").is_none(),
        "data must be inside notification, not top-level"
    );
}

#[test]
fn payload_minimal_has_title_body_and_modal_tap() {
    // Hard cut: `tap` is always present in `notification.data` — the SW
    // routes off it and the wake-push payload must match byte-for-byte
    // for tag-replace to dedupe cleanly.
    let payload = build_push_payload("Hi", "There", None, None, None, None, &Tap::Modal, None);
    assert_eq!(payload["notification"]["title"], "Hi");
    assert_eq!(payload["notification"]["body"], "There");
    let data = &payload["notification"]["data"];
    assert!(data.get("notification_id").is_none());
    assert!(data.get("app_id").is_none());
    assert!(data.get("thread_id").is_none());
    assert!(data.get("event_id").is_none());
    assert_eq!(data["tap"], serde_json::json!({"kind": "modal"}));
}

#[test]
fn payload_includes_thread_id_for_deep_link() {
    let tid = uuid::Uuid::parse_str("12345678-1234-5678-1234-567812345678").unwrap();
    let payload = build_push_payload(
        "Claude is asking",
        "Pick one",
        None,
        None,
        Some(tid),
        None,
        &Tap::Modal,
        None,
    );
    assert_eq!(
        payload["notification"]["data"]["thread_id"],
        tid.to_string()
    );
}

#[test]
fn payload_omits_thread_id_when_link_absent() {
    // Regression guard: the SW relies on `if (data.thread_id)` — never emit
    // the key as a literal string "null" or empty value.
    let payload = build_push_payload(
        "Hi",
        "There",
        None,
        Some("app-x"),
        None,
        None,
        &Tap::Modal,
        None,
    );
    let data = &payload["notification"]["data"];
    assert!(data.get("thread_id").is_none());
    assert_eq!(data["app_id"], "app-x");
}

#[test]
fn payload_carries_all_fields_when_provided() {
    let nid = uuid::Uuid::new_v4();
    let tid = uuid::Uuid::new_v4();
    let eid = uuid::Uuid::new_v4();
    let payload = build_push_payload(
        "T",
        "B",
        Some(nid),
        Some("the-app"),
        Some(tid),
        Some(eid),
        &Tap::Modal,
        None,
    );
    let data = &payload["notification"]["data"];
    assert_eq!(data["notification_id"], nid.to_string());
    assert_eq!(data["app_id"], "the-app");
    assert_eq!(data["thread_id"], tid.to_string());
    assert_eq!(data["event_id"], eid.to_string());
}

#[test]
fn payload_includes_event_id_when_set() {
    let tid = uuid::Uuid::new_v4();
    let eid = uuid::Uuid::new_v4();
    let tap = nav_thread(&tid.to_string(), Some(&eid.to_string()));
    let payload = build_push_payload(
        "Claude is asking",
        "Pick one",
        None,
        None,
        Some(tid),
        Some(eid),
        &tap,
        None,
    );
    let data = &payload["notification"]["data"];
    assert_eq!(data["event_id"], eid.to_string());
    assert_eq!(data["thread_id"], tid.to_string());
}

#[test]
fn payload_omits_event_id_when_not_provided() {
    let tid = uuid::Uuid::new_v4();
    let tap = nav_thread(&tid.to_string(), None);
    let payload = build_push_payload("Hi", "There", None, None, Some(tid), None, &tap, None);
    assert!(payload["notification"]["data"].get("event_id").is_none());
}

#[test]
fn payload_always_includes_tap_even_when_modal_default() {
    let payload = build_push_payload("Hi", "There", None, None, None, None, &Tap::Modal, None);
    assert_eq!(
        payload["notification"]["data"]["tap"],
        serde_json::json!({"kind": "modal"})
    );
}

#[test]
fn payload_includes_navigate_tap_for_cta_app() {
    let nid = uuid::Uuid::new_v4();
    let tap = nav_app("habit-tracker");
    let payload = build_push_payload(
        "Time to check in",
        "Log today's habits",
        Some(nid),
        Some("habit-tracker"),
        None,
        None,
        &tap,
        None,
    );
    let tap_val = &payload["notification"]["data"]["tap"];
    assert_eq!(tap_val["kind"], "navigate");
    assert_eq!(tap_val["to"]["target"], "app");
    assert_eq!(tap_val["to"]["app_id"], "habit-tracker");
}

#[test]
fn wake_payload_carries_wake_flag_plus_original_content() {
    // Layer 3 of the macOS-Chrome partial-wedge mitigation (see
    // system-knowhow/notifications.md §4.5). The `wake: true` flag sits
    // at TOP LEVEL (sibling to `web_push`/`notification`), NOT inside the
    // notification object — Safari ignores unknown top-level fields, and
    // wake pushes never reach Safari anyway (filtered by is_mac_chromium).
    // The SW gates on `data.wake` to skip the visible re-pop while still
    // calling showNotification so Chrome counts the push as user-visible.
    use crate::scheduler::notifications::Notification;
    let nid = uuid::Uuid::new_v4();
    let tid = uuid::Uuid::new_v4();
    let eid = uuid::Uuid::new_v4();
    let n = Notification {
        id: nid,
        task_id: None,
        app_id: Some("habit-tracker".into()),
        thread_id: Some(tid),
        event_id: Some(eid),
        title: "Claude is asking".into(),
        message: "Pick one".into(),
        read: false,
        created_at: chrono::Utc::now(),
        tap: nav_thread(&tid.to_string(), Some(&eid.to_string())),
    };
    let payload = build_wake_payload(&n, None);
    assert_eq!(payload["wake"], true);
    assert_eq!(payload["web_push"], 8030);
    let notif = &payload["notification"];
    assert_eq!(notif["title"], "Claude is asking");
    assert_eq!(notif["body"], "Pick one");
    let data = &notif["data"];
    assert_eq!(data["notification_id"], nid.to_string());
    assert_eq!(data["app_id"], "habit-tracker");
    assert_eq!(data["thread_id"], tid.to_string());
    assert_eq!(data["event_id"], eid.to_string());
    assert_eq!(data["tap"]["kind"], "navigate");
    assert_eq!(data["tap"]["to"]["target"], "thread");
    assert_eq!(data["tap"]["to"]["id"], tid.to_string());
    assert_eq!(data["tap"]["to"]["event_id"], eid.to_string());
}

#[test]
fn payload_includes_navigate_tap_for_cta_thread() {
    let nid = uuid::Uuid::new_v4();
    let tid = uuid::Uuid::new_v4();
    let tap = nav_thread(&tid.to_string(), None);
    let payload = build_push_payload(
        "Claude is asking",
        "Pick one",
        Some(nid),
        None,
        Some(tid),
        None,
        &tap,
        None,
    );
    let tap_val = &payload["notification"]["data"]["tap"];
    assert_eq!(tap_val["kind"], "navigate");
    assert_eq!(tap_val["to"]["target"], "thread");
    assert_eq!(tap_val["to"]["id"], tid.to_string());
}

#[test]
fn declarative_navigate_falls_back_to_scope_relative_query_when_scope_missing() {
    // The iOS-consumed `notification.navigate` field MUST be a cross-document
    // (query-string) URL, NOT a hash-only one. iOS Safari's declarative-push
    // handler reuses the already-open PWA window and a same-document (hash-only)
    // navigation is NOT applied to it — the OS just focuses the window, the URL
    // never updates, and the page-side hash router finds nothing to route
    // ("tap nav to thread only focuses the app"). A query string changes the
    // document, forcing a real navigation iOS actually performs. See
    // system-knowhow/notifications.md §4.5.
    //
    // Legacy subscriptions created before scope_url was recorded still fan out.
    // The next page load refreshes them with scope_url; until then keep the
    // previous relative fallback instead of breaking delivery.
    let nid = uuid::Uuid::new_v4();
    let tid = uuid::Uuid::new_v4();
    let eid = uuid::Uuid::new_v4();
    let tap = nav_thread(&tid.to_string(), Some(&eid.to_string()));
    let payload = build_push_payload("T", "B", Some(nid), None, Some(tid), Some(eid), &tap, None);
    let nav = payload["notification"]["navigate"].as_str().unwrap();
    assert!(
        nav.starts_with('?') && !nav.starts_with('/'),
        "legacy iOS navigate fallback must remain a scope-relative query URL, got: {nav}"
    );
    assert!(nav.contains(&format!("notification={}", nid)));
    assert!(nav.contains(&format!("thread={}", tid)));
    assert!(nav.contains(&format!("event={}", eid)));
    assert!(
        nav.contains("tap="),
        "navigate must carry encoded tap for non-modal kinds"
    );
}

#[test]
fn declarative_navigate_uses_absolute_scope_url_for_ios() {
    // The regression that made hot AND cold iOS taps consistently no-op:
    // WebKit/APNs may not apply a bare query-only `notification.navigate`.
    // Persist the SW scope when subscribing and emit the concrete absolute URL.
    let nid = uuid::Uuid::new_v4();
    let tid = uuid::Uuid::new_v4();
    let eid = uuid::Uuid::new_v4();
    let tap = nav_thread(&tid.to_string(), Some(&eid.to_string()));
    let payload = build_push_payload(
        "T",
        "B",
        Some(nid),
        None,
        Some(tid),
        Some(eid),
        &tap,
        Some("https://lucidos.test/dev/"),
    );
    let nav = payload["notification"]["navigate"].as_str().unwrap();
    assert!(
        nav.starts_with("https://lucidos.test/dev/?"),
        "iOS navigate must be absolute and preserve the gateway workspace scope, got: {nav}"
    );
    assert!(nav.contains(&format!("notification={}", nid)));
    assert!(nav.contains(&format!("thread={}", tid)));
    assert!(nav.contains(&format!("event={}", eid)));
    assert!(
        nav.contains("tap="),
        "navigate must carry encoded tap for non-modal kinds"
    );
}

#[test]
fn declarative_navigate_normalizes_scope_url_before_query_append() {
    let nid = uuid::Uuid::new_v4();
    let payload = build_push_payload(
        "T",
        "B",
        Some(nid),
        None,
        None,
        None,
        &Tap::Modal,
        Some("https://lucidos.test/dev?stale=1#frag"),
    );
    assert_eq!(
        payload["notification"]["navigate"],
        format!("https://lucidos.test/dev/?notification={nid}")
    );
}

#[test]
fn declarative_navigate_omits_tap_param_for_modal_kind() {
    // Modal-kind taps still want the deep-link to open the inbox modal via
    // `notification=…`, but the `tap=` param is redundant: the page-side
    // dispatcher defaults missing tap to modal. Keeps URLs short.
    let nid = uuid::Uuid::new_v4();
    let payload = build_push_payload("T", "B", Some(nid), None, None, None, &Tap::Modal, None);
    let nav = payload["notification"]["navigate"].as_str().unwrap();
    assert!(nav.contains(&format!("notification={}", nid)));
    assert!(
        !nav.contains("tap="),
        "modal taps must NOT include tap= in nav URL"
    );
}

#[test]
fn declarative_navigate_root_when_no_params() {
    // Defensive: a push with no notification_id / thread_id / event_id and
    // modal tap has nothing to deep-link. Both navigate URLs fall back to "."
    // — a scope-relative ref that resolves to the workspace (`/<slug>/`) root
    // on iOS. A bare "/" would escape to the origin root (the gateway picker),
    // the same multi-workspace trap the parametrized URLs avoid.
    let payload = build_push_payload("T", "B", None, None, None, None, &Tap::Modal, None);
    assert_eq!(payload["notification"]["navigate"], ".");
    assert_eq!(payload["notification"]["data"]["navigate"], ".");
}

#[test]
fn declarative_navigate_scope_root_when_no_params_and_scope_present() {
    let payload = build_push_payload(
        "T",
        "B",
        None,
        None,
        None,
        None,
        &Tap::Modal,
        Some("https://lucidos.test/dev/"),
    );
    assert_eq!(
        payload["notification"]["navigate"],
        "https://lucidos.test/dev/"
    );
    assert_eq!(payload["notification"]["data"]["navigate"], ".");
}

#[test]
fn declarative_notification_tag_carries_notification_id_for_dedup() {
    // Mirrors the SW's existing tag behavior: same notification_id replaces
    // (renotify) the existing OS-level notification rather than stacking
    // duplicates. Engine produces the tag now so Safari's declarative path
    // gets the same dedup as the SW path.
    let nid = uuid::Uuid::new_v4();
    let payload = build_push_payload("T", "B", Some(nid), None, None, None, &Tap::Modal, None);
    assert_eq!(payload["notification"]["tag"], nid.to_string());
}

#[test]
fn declarative_notification_tag_falls_back_to_default_when_no_id() {
    // Legacy callers without a notification_id still need a tag — the
    // engine fills in "lucidos-notification" so the OS dedup channel
    // exists.
    let payload = build_push_payload("T", "B", None, None, None, None, &Tap::Modal, None);
    assert_eq!(payload["notification"]["tag"], "lucidos-notification");
}

#[test]
fn declarative_notification_data_carries_hash_navigate_url_for_chrome_sw() {
    // The Chrome SW notificationclick handler reads the navigate URL off
    // `event.notification.data` — duplicate it inside `notification.data` so
    // the click handler doesn't have to rebuild the URL from the individual
    // fields. Safari ignores `data`; Chrome's SW reads it.
    //
    // The two URLs are deliberately DIFFERENT forms: `data.navigate` (Chrome
    // SW `client.navigate()`) stays a HASH URL so a warm tap is a same-document
    // change (no reload), while `notification.navigate` (iOS declarative) is an
    // absolute QUERY URL so iOS cold/hot launches do not depend on WebKit/APNs
    // accepting a query-only relative string. Same params, different carrier.
    // See system-knowhow/notifications.md §4.5.
    let nid = uuid::Uuid::new_v4();
    let tid = uuid::Uuid::new_v4();
    let tap = nav_thread(&tid.to_string(), None);
    let payload = build_push_payload(
        "T",
        "B",
        Some(nid),
        None,
        Some(tid),
        None,
        &tap,
        Some("https://lucidos.test/dev/"),
    );
    let nav_ios = payload["notification"]["navigate"].as_str().unwrap();
    let nav_sw = payload["notification"]["data"]["navigate"]
        .as_str()
        .unwrap();
    assert!(
        nav_ios.starts_with("https://lucidos.test/dev/?"),
        "iOS navigate must be an absolute scoped query URL, got: {nav_ios}"
    );
    assert!(
        nav_sw.starts_with('#') && !nav_sw.starts_with('/'),
        "Chrome SW navigate must be a scope-relative hash URL, got: {nav_sw}"
    );
    // Same deep-link params despite the different URL carriers.
    let ios_query = nav_ios
        .split_once('?')
        .map(|(_, query)| query)
        .expect("absolute iOS navigate must carry query params");
    assert_eq!(ios_query, nav_sw.trim_start_matches('#'));
    assert!(nav_sw.contains(&format!("notification={}", nid)));
    assert!(nav_sw.contains(&format!("thread={}", tid)));
}

// §2 Step A — decide_push_allowed unit tests. The matrix says the
// engine sends the OS push iff no candidate device is "active" right
// now; multi-tab same-device pongs OR within the device.

fn pong(device_id: &str, is_active: bool) -> PresencePong {
    PresencePong {
        device_id: device_id.into(),
        is_active,
        focused_thread_id: None,
        event_in_viewport: false,
    }
}

#[test]
fn s2_step_a_no_candidates_means_push_allowed() {
    // Engine resolved zero candidate devices → skip the PresenceCheck
    // and set push_allowed=true directly. The function still has to
    // handle the empty-vec input cleanly because the same code path
    // collects pongs that timed out.
    assert!(decide_push_allowed(&[]));
}

#[test]
fn s3_run_presence_check_when_sse_connected_even_with_no_candidate() {
    // The regression: iOS suspends the 30s heartbeat while the PWA is
    // foregrounded, so device_presence has ZERO candidates even though the
    // EventSource is open and the page would pong is_active. The live
    // SSE-connection count must still trigger the PresenceCheck (>0), so the
    // active page gets a chance to suppress the OS push.
    assert!(
        expected_pong_count(1, 0) > 0,
        "a connected SSE page with a stale heartbeat must still run the PresenceCheck"
    );
}

#[test]
fn s3_skip_presence_check_when_nobody_connected_and_no_candidate() {
    // The "phone in your pocket" case — no open SSE stream, no fresh
    // heartbeat. Nobody to pong → skip the protocol and push directly.
    assert_eq!(expected_pong_count(0, 0), 0);
}

#[test]
fn s3_expected_pong_count_is_max_of_both_signals() {
    // More connected tabs than fresh heartbeats (the common case after a
    // suspended heartbeat resumes): wait for every connected page.
    assert_eq!(expected_pong_count(2, 1), 2);
    // Inverse failure mode: a page heartbeated within 120s but its SSE just
    // dropped — counting the candidate keeps the deadline waiting for it.
    assert_eq!(expected_pong_count(1, 3), 3);
}

#[test]
fn s2_step_a_active_pong_means_push_not_allowed() {
    let pongs = [pong("dev-1", true)];
    assert!(!decide_push_allowed(&pongs));
}

#[test]
fn s2_step_a_only_inactive_pongs_means_push_allowed() {
    // Every connected page is in the background — push to anyone with
    // a subscription so the user is notified.
    let pongs = [pong("dev-1", false), pong("dev-2", false)];
    assert!(decide_push_allowed(&pongs));
}

#[test]
fn s2_multi_tab_one_active_one_hidden_treats_device_as_active() {
    // Two tabs on the same browser → both pong with the same
    // device_id. One active, one not. The OR-across-devices test (any
    // active) makes this a non-push case without any extra grouping.
    let pongs = [pong("dev-1", false), pong("dev-1", true)];
    assert!(!decide_push_allowed(&pongs));
}

#[test]
fn s4_5_ua_predicate_matches_chrome_on_macos() {
    let ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
              AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 \
              Safari/537.36";
    assert!(is_mac_chromium(ua));
}

#[test]
fn s4_5_ua_predicate_matches_edge_on_macos() {
    // Edge on macOS is Chromium-based and includes both `Chrome/` and
    // `Edg/` tokens — must match.
    let ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
              AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 \
              Safari/537.36 Edg/120.0.0.0";
    assert!(is_mac_chromium(ua));
}

#[test]
fn s4_5_ua_predicate_excludes_safari_on_macos() {
    // Safari has the `navigate` option (Layer 1) shipped in 18.5+; the
    // wedge does not bite, and the wake-push is pure waste.
    let ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
              AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 \
              Safari/605.1.15";
    assert!(!is_mac_chromium(ua));
}

#[test]
fn s4_5_ua_predicate_excludes_chrome_on_windows() {
    // The wedge is macOS-only — Chrome on Windows / Linux doesn't have
    // this dispatcher bug.
    let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
              (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
    assert!(!is_mac_chromium(ua));
}

#[test]
fn s4_5_ua_predicate_excludes_chrome_on_ios() {
    // Chrome on iOS (CriOS) is a WebKit wrapper, different render engine,
    // different bug surface. Same `Macintosh`-token concern as the
    // frontend predicate — iPhone/iPad/iPod tokens must veto.
    let ua = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) \
              AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/120.0.0.0 \
              Mobile/15E148 Safari/604.1";
    assert!(!is_mac_chromium(ua));
}

#[test]
fn s4_5_ua_predicate_excludes_firefox_on_macos() {
    let ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:120.0) \
              Gecko/20100101 Firefox/120.0";
    assert!(!is_mac_chromium(ua));
}

#[test]
fn s4_5_ua_predicate_excludes_empty_ua() {
    // Devices registered without a UA (legacy rows, weird clients) — be
    // conservative, skip the wake. No-op falls back to "next genuine push
    // drains the queued click" — same as a wake-task aborted at engine
    // shutdown (Layer 3 is best-effort by design).
    assert!(!is_mac_chromium(""));
}

fn sub_with_device(device_id: &str) -> PushSubscription {
    PushSubscription {
        endpoint: format!("https://example.com/push/{}", device_id),
        p256dh: "p256dh-test".into(),
        auth: "auth-test".into(),
        device_id: Some(device_id.into()),
        scope_url: None,
    }
}

fn sub_without_device() -> PushSubscription {
    PushSubscription {
        endpoint: "https://example.com/push/anon".into(),
        p256dh: "p256dh-test".into(),
        auth: "auth-test".into(),
        device_id: None,
        scope_url: None,
    }
}

const CHROME_MAC_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
const SAFARI_MAC_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
    AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15";

#[test]
fn s4_5_pick_wake_targets_empty_input() {
    let targets = pick_mac_chromium_wake_targets(&[]);
    assert!(targets.is_empty());
}

#[test]
fn s4_5_pick_wake_targets_only_mac_chromium() {
    let subs = vec![
        (
            sub_with_device("dev-chrome"),
            Some(CHROME_MAC_UA.to_string()),
        ),
        (
            sub_with_device("dev-safari"),
            Some(SAFARI_MAC_UA.to_string()),
        ),
    ];
    let targets = pick_mac_chromium_wake_targets(&subs);
    assert_eq!(targets, vec!["dev-chrome".to_string()]);
}

#[test]
fn s4_5_pick_wake_targets_skips_no_device_id() {
    // Legacy rows without device_id can't be targeted by
    // `send_wake_push_to_device` (it filters by device_id). Skip
    // rather than wake the whole world.
    let subs = vec![(sub_without_device(), Some(CHROME_MAC_UA.to_string()))];
    let targets = pick_mac_chromium_wake_targets(&subs);
    assert!(targets.is_empty());
}

#[test]
fn s4_5_pick_wake_targets_skips_no_ua() {
    // Devices registered before user_agent was captured (or by clients
    // that don't send it) — be conservative, no wake.
    let subs = vec![(sub_with_device("dev-unknown"), None)];
    let targets = pick_mac_chromium_wake_targets(&subs);
    assert!(targets.is_empty());
}

#[test]
fn s4_5_pick_wake_targets_dedupes_multi_tab() {
    // Two tabs of the same browser = two subscriptions with the same
    // device_id. Wake-push fans out per subscription inside
    // `send_wake_push_to_device`, so the per-device wake task should
    // only spawn once — otherwise the same SW gets two wakes 3s apart
    // for the same notification, wasting half the budget.
    let subs = vec![
        (
            sub_with_device("dev-chrome"),
            Some(CHROME_MAC_UA.to_string()),
        ),
        (
            sub_with_device("dev-chrome"),
            Some(CHROME_MAC_UA.to_string()),
        ),
    ];
    let targets = pick_mac_chromium_wake_targets(&subs);
    assert_eq!(targets, vec!["dev-chrome".to_string()]);
}

#[test]
fn s4_5_wake_skipped_when_notification_already_read() {
    // The resurrect-after-read bug: the wake fires MAC_CHROMIUM_WAKE_DELAY
    // after the real push, but the user may tap the original banner inside
    // that window — `notificationclick` closes it and marks it read. If the
    // wake still fired it would re-pop the already-handled notification as a
    // fresh unread banner (the user-reported "same push twice"). A read
    // notification means the tap landed (SW wasn't wedged), so the wake has
    // no job. `send_wake_push_to_device` re-fetches the live read state at
    // fire time and gates on this predicate.
    use crate::scheduler::notifications::Notification;
    let mut n = Notification {
        id: uuid::Uuid::new_v4(),
        task_id: None,
        app_id: None,
        thread_id: Some(uuid::Uuid::new_v4()),
        event_id: None,
        title: "Claude is asking".into(),
        message: "Pick one".into(),
        read: false,
        created_at: chrono::Utc::now(),
        tap: Tap::Modal,
    };
    assert!(
        wake_still_needed(&n),
        "an unread notification still needs the wake — the tap may have been swallowed by a wedged SW"
    );
    n.read = true;
    assert!(
        !wake_still_needed(&n),
        "a read notification means the tap already drained — skip the wake so it isn't resurrected"
    );
}

#[tokio::test(start_paused = true)]
async fn s3_deadline_long_enough_for_realistic_ios_cellular_pong() {
    // Regression for "iOS PWA push fires alongside the in-app toast":
    // the page renders the toast synchronously when the PresenceCheck
    // SSE lands, but its pong POST round-trips back over cellular /
    // Tailscale slower than the engine's deadline. The engine then
    // collects zero active pongs and fans out the OS push too.
    //
    // Two RTT bands appear in real traces:
    //
    // - steady-state cellular / Tailscale relay: 400–800 ms. Covered
    //   by `DEADLINE_MS=1000` (legacy).
    // - first-packet-after-radio-idle on Tailscale: routinely
    //   1100–1600 ms, occasionally up to ~1800 ms. NOT covered by
    //   the legacy 1000 ms deadline — this is the case the user
    //   reported (push fires on top of the toast on a foregrounded
    //   iOS PWA over Tailscale).
    //
    // Simulating the worst realistic band keeps the deadline pinned
    // to "covers the user's actual network", not "covers the steady-
    // state average". 1500 ms is the median of the wake-up band;
    // bumping DEADLINE_MS to 2000 ms leaves ~500 ms headroom.
    //
    // `start_paused` + the auto-advancing tokio test clock lets us
    // assert the timing precisely without sleeping in wall time.
    use crate::api::presence_pong::{PresencePongRequest, PresenceTracker};
    use std::time::Duration;

    const SIMULATED_RTT_MS: u64 = 1500;

    let tracker = PresenceTracker::new();
    let nid = uuid::Uuid::new_v4();
    let notify = tracker.expect(nid, 1);

    let tracker_clone = tracker.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(SIMULATED_RTT_MS)).await;
        tracker_clone.record(PresencePongRequest {
            notification_id: nid,
            device_id: "ios-cellular".into(),
            is_active: true,
            focused_thread_id: None,
            event_in_viewport: false,
        });
    });

    let _ =
        tokio::time::timeout(Duration::from_millis(DEADLINE_MS as u64), notify.notified()).await;

    let pongs = tracker.collect(nid);
    assert!(
        !decide_push_allowed(&pongs),
        "DEADLINE_MS={} is too short for a {} ms iOS-cellular pong — \
         engine times out before the page can answer, so push fires \
         on top of the in-app toast. Got pongs={:?}",
        DEADLINE_MS,
        SIMULATED_RTT_MS,
        pongs
            .iter()
            .map(|p| (p.device_id.clone(), p.is_active))
            .collect::<Vec<_>>(),
    );
}
