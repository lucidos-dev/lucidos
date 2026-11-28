use super::*;

use crate::scheduler::notifications::{NavigateTarget, NavigateUi, Tap};

fn nav_thread(id: &str, event_id: Option<&str>) -> Tap {
    Tap::Navigate {
        to: Box::new(NavigateUi {
            target: NavigateTarget::Thread,
            id: Some(id.to_string()),
            event_id: event_id.map(str::to_string),
            ..Default::default()
        }),
    }
}

fn nav_app(app_id: &str) -> Tap {
    Tap::Navigate {
        to: Box::new(NavigateUi {
            target: NavigateTarget::App,
            app_id: Some(app_id.to_string()),
            ..Default::default()
        }),
    }
}

#[test]
fn declarative_envelope_has_web_push_magic_and_notification_object() {
    let payload = build_push_payload(
        None,
        "Hi",
        "There",
        None,
        None,
        None,
        None,
        &Tap::Modal,
        None,
        None,
    );
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
fn badge_scope_reads_who_served_the_install() {
    assert_eq!(
        badge_scope(Some("https://lucidos.test/dev/")),
        BadgeScope::Gateway
    );
    assert_eq!(
        badge_scope(Some("http://192.0.2.10:5251/myws/")),
        BadgeScope::Gateway
    );
    assert_eq!(
        badge_scope(Some("https://lucidos.test/~/")),
        BadgeScope::Gateway
    );

    assert_eq!(
        badge_scope(Some("https://gateway-a.test/dev@cloud/")),
        BadgeScope::Relayed
    );

    assert_eq!(
        badge_scope(Some("https://lucidos.test:5250/")),
        BadgeScope::Own
    );
    assert_eq!(badge_scope(Some("http://localhost:5250/")), BadgeScope::Own);
    assert_eq!(badge_scope(None), BadgeScope::Own);
    assert_eq!(badge_scope(Some("not a url")), BadgeScope::Own);
    assert_eq!(badge_scope(Some("")), BadgeScope::Own);
}

#[test]
fn a_relayed_install_leaves_the_badge_alone() {
    assert_eq!(
        app_badge_for(Some("https://gateway-a.test/dev@cloud/"), Some(2), Some(9)),
        None
    );
}

#[test]
fn app_badge_is_the_aggregate_only_for_a_gateway_install() {
    assert_eq!(
        app_badge_for(Some("https://lucidos.test/dev/"), Some(2), Some(9)),
        Some(9)
    );
    assert_eq!(
        app_badge_for(Some("https://lucidos.test/dev/"), Some(2), None),
        Some(2)
    );
    assert_eq!(
        app_badge_for(Some("https://lucidos.test:5250/"), Some(2), Some(9)),
        Some(2)
    );
    assert_eq!(app_badge_for(None, Some(2), Some(9)), Some(2));
    assert_eq!(
        app_badge_for(Some("https://lucidos.test/dev/"), None, None),
        None
    );
}

#[test]
fn gateway_total_is_read_from_the_control_answer() {
    assert_eq!(parse_unread_total(r#"{"total":7}"#), Some(7));
    assert_eq!(parse_unread_total(r#"{"total":0}"#), Some(0));
    assert_eq!(parse_unread_total(r#"{"total":"7"}"#), None);
    assert_eq!(parse_unread_total(r#"{"workspaces":[]}"#), None);
    assert_eq!(parse_unread_total("not json"), None);
}

#[test]
fn payload_app_badge_reflects_the_resolved_badge_count() {
    let payload = build_push_payload(
        None,
        "T",
        "B",
        None,
        None,
        None,
        None,
        &Tap::Modal,
        None,
        Some(3),
    );
    assert_eq!(payload["app_badge"], 3);

    let cleared = build_push_payload(
        None,
        "T",
        "B",
        None,
        None,
        None,
        None,
        &Tap::Modal,
        None,
        Some(0),
    );
    assert_eq!(cleared["app_badge"], 0);

    let absent = build_push_payload(
        None,
        "T",
        "B",
        None,
        None,
        None,
        None,
        &Tap::Modal,
        None,
        None,
    );
    assert!(absent.get("app_badge").is_none());
}

#[test]
fn payload_minimal_has_title_body_and_modal_tap() {
    let payload = build_push_payload(
        None,
        "Hi",
        "There",
        None,
        None,
        None,
        None,
        &Tap::Modal,
        None,
        None,
    );
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
        None,
        "Claude is asking",
        "Pick one",
        None,
        None,
        Some(tid),
        None,
        &Tap::Modal,
        None,
        None,
    );
    assert_eq!(
        payload["notification"]["data"]["thread_id"],
        tid.to_string()
    );
}

#[test]
fn payload_omits_thread_id_when_link_absent() {
    let payload = build_push_payload(
        None,
        "Hi",
        "There",
        None,
        Some("app-x"),
        None,
        None,
        &Tap::Modal,
        None,
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
        None,
        "T",
        "B",
        Some(nid),
        Some("the-app"),
        Some(tid),
        Some(eid),
        &Tap::Modal,
        None,
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
        None,
        "Claude is asking",
        "Pick one",
        None,
        None,
        Some(tid),
        Some(eid),
        &tap,
        None,
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
    let payload = build_push_payload(
        None,
        "Hi",
        "There",
        None,
        None,
        Some(tid),
        None,
        &tap,
        None,
        None,
    );
    assert!(payload["notification"]["data"].get("event_id").is_none());
}

#[test]
fn payload_always_includes_tap_even_when_modal_default() {
    let payload = build_push_payload(
        None,
        "Hi",
        "There",
        None,
        None,
        None,
        None,
        &Tap::Modal,
        None,
        None,
    );
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
        None,
        "Time to check in",
        "Log today's habits",
        Some(nid),
        Some("habit-tracker"),
        None,
        None,
        &tap,
        None,
        None,
    );
    let tap_val = &payload["notification"]["data"]["tap"];
    assert_eq!(tap_val["kind"], "navigate");
    assert_eq!(tap_val["to"]["target"], "app");
    assert_eq!(tap_val["to"]["app_id"], "habit-tracker");
}

#[test]
fn wake_payload_carries_wake_flag_plus_original_content() {
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
    let payload = build_wake_payload(None, &n, None, None, None);
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
        None,
        "Claude is asking",
        "Pick one",
        Some(nid),
        None,
        Some(tid),
        None,
        &tap,
        None,
        None,
    );
    let tap_val = &payload["notification"]["data"]["tap"];
    assert_eq!(tap_val["kind"], "navigate");
    assert_eq!(tap_val["to"]["target"], "thread");
    assert_eq!(tap_val["to"]["id"], tid.to_string());
}

#[test]
fn declarative_navigate_falls_back_to_scope_relative_query_when_scope_missing() {
    let nid = uuid::Uuid::new_v4();
    let tid = uuid::Uuid::new_v4();
    let eid = uuid::Uuid::new_v4();
    let tap = nav_thread(&tid.to_string(), Some(&eid.to_string()));
    let payload = build_push_payload(
        None,
        "T",
        "B",
        Some(nid),
        None,
        Some(tid),
        Some(eid),
        &tap,
        None,
        None,
    );
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
    let nid = uuid::Uuid::new_v4();
    let tid = uuid::Uuid::new_v4();
    let eid = uuid::Uuid::new_v4();
    let tap = nav_thread(&tid.to_string(), Some(&eid.to_string()));
    let payload = build_push_payload(
        None,
        "T",
        "B",
        Some(nid),
        None,
        Some(tid),
        Some(eid),
        &tap,
        Some("https://lucidos.test/dev/"),
        None,
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
        None,
        "T",
        "B",
        Some(nid),
        None,
        None,
        None,
        &Tap::Modal,
        Some("https://lucidos.test/dev?stale=1#frag"),
        None,
    );
    assert_eq!(
        payload["notification"]["navigate"],
        format!("https://lucidos.test/dev/?notification={nid}&restore_generation=none")
    );
}

#[test]
fn declarative_navigate_omits_tap_param_for_modal_kind() {
    let nid = uuid::Uuid::new_v4();
    let payload = build_push_payload(
        None,
        "T",
        "B",
        Some(nid),
        None,
        None,
        None,
        &Tap::Modal,
        None,
        None,
    );
    let nav = payload["notification"]["navigate"].as_str().unwrap();
    assert!(nav.contains(&format!("notification={}", nid)));
    assert!(
        !nav.contains("tap="),
        "modal taps must NOT include tap= in nav URL"
    );
}

#[test]
fn declarative_navigate_root_when_no_params() {
    let payload = build_push_payload(
        None,
        "T",
        "B",
        None,
        None,
        None,
        None,
        &Tap::Modal,
        None,
        None,
    );
    assert_eq!(payload["notification"]["navigate"], ".");
    assert_eq!(payload["notification"]["data"]["navigate"], ".");
}

#[test]
fn a_relayed_subscription_taps_through_the_outside_origin() {
    let tid = uuid::Uuid::new_v4();
    let tap = nav_thread(&tid.to_string(), None);
    let payload = build_push_payload(
        None,
        "T",
        "B",
        None,
        None,
        Some(tid),
        None,
        &tap,
        Some("https://gateway-a.test/dev@cloud/"),
        None,
    );
    let nav = payload["notification"]["navigate"].as_str().unwrap();
    assert!(
        nav.starts_with("https://gateway-a.test/dev@cloud/?"),
        "iOS navigate must keep the outside prefix, got: {nav}"
    );
    assert!(payload["notification"]["data"]["navigate"]
        .as_str()
        .unwrap()
        .starts_with('#'));
}

#[test]
fn declarative_navigate_scope_root_when_no_params_and_scope_present() {
    let payload = build_push_payload(
        None,
        "T",
        "B",
        None,
        None,
        None,
        None,
        &Tap::Modal,
        Some("https://lucidos.test/dev/"),
        None,
    );
    assert_eq!(
        payload["notification"]["navigate"],
        "https://lucidos.test/dev/"
    );
    assert_eq!(payload["notification"]["data"]["navigate"], ".");
}

#[test]
fn declarative_notification_tag_carries_notification_id_for_dedup() {
    let nid = uuid::Uuid::new_v4();
    let payload = build_push_payload(
        None,
        "T",
        "B",
        Some(nid),
        None,
        None,
        None,
        &Tap::Modal,
        None,
        None,
    );
    assert_eq!(payload["notification"]["tag"], nid.to_string());
}

#[test]
fn declarative_notification_tag_falls_back_to_default_when_no_id() {
    let payload = build_push_payload(
        None,
        "T",
        "B",
        None,
        None,
        None,
        None,
        &Tap::Modal,
        None,
        None,
    );
    assert_eq!(payload["notification"]["tag"], "lucidos-notification");
}

#[test]
fn declarative_notification_data_carries_hash_navigate_url_for_chrome_sw() {
    let nid = uuid::Uuid::new_v4();
    let tid = uuid::Uuid::new_v4();
    let tap = nav_thread(&tid.to_string(), None);
    let payload = build_push_payload(
        None,
        "T",
        "B",
        Some(nid),
        None,
        Some(tid),
        None,
        &tap,
        Some("https://lucidos.test/dev/"),
        None,
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
    let ios_query = nav_ios
        .split_once('?')
        .map(|(_, query)| query)
        .expect("absolute iOS navigate must carry query params");
    assert_eq!(ios_query, nav_sw.trim_start_matches('#'));
    assert!(nav_sw.contains(&format!("notification={}", nid)));
    assert!(nav_sw.contains(&format!("thread={}", tid)));
}

fn build_real_web_push_message(payload: &str) -> Result<(), web_push::WebPushError> {
    use base64::Engine as _;
    use p256::ecdsa::SigningKey;
    use p256::elliptic_curve::rand_core::OsRng;

    let signing_key = SigningKey::random(&mut OsRng);
    let point = signing_key.verifying_key().to_encoded_point(false);
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let sub_info = web_push::SubscriptionInfo::new(
        "https://example.com/push/test-endpoint".to_string(),
        b64.encode(point.as_bytes()),
        b64.encode([7u8; 16]),
    );

    let mut msg_builder = web_push::WebPushMessageBuilder::new(&sub_info);
    msg_builder.set_payload(web_push::ContentEncoding::Aes128Gcm, payload.as_bytes());
    msg_builder.build().map(|_| ())
}

fn overflowing_notification_body(char_len: usize) -> String {
    let unit = "The nightly pipeline finished green across every shard. ";
    let mut body = String::new();
    while body.chars().count() + unit.chars().count() <= char_len {
        body.push_str(unit);
    }
    while body.chars().count() < char_len {
        body.push('x');
    }
    assert_eq!(body.chars().count(), char_len);
    body
}

fn real_world_overflow_payload(body: &str) -> serde_json::Value {
    let nid = uuid::Uuid::new_v4();
    let tid = uuid::Uuid::new_v4();
    let eid = uuid::Uuid::new_v4();
    let tap = nav_thread(&tid.to_string(), Some(&eid.to_string()));
    build_push_payload_fitted(
        None,
        "Nightly pipeline: GREEN ✅",
        body,
        Some(nid),
        Some("ci-watch"),
        Some(tid),
        Some(eid),
        &tap,
        Some("https://lucidos.test/dev/"),
        Some(7),
        None,
    )
}

#[test]
fn s4_5_crate_ceiling_constant_matches_what_build_actually_enforces() {
    let at_ceiling = "a".repeat(MAX_PUSH_PAYLOAD_BYTES);
    assert!(
        build_real_web_push_message(&at_ceiling).is_ok(),
        "MAX_PUSH_PAYLOAD_BYTES is below what the crate accepts"
    );
    let over_ceiling = "a".repeat(MAX_PUSH_PAYLOAD_BYTES + 1);
    assert!(
        matches!(
            build_real_web_push_message(&over_ceiling),
            Err(web_push::WebPushError::PayloadTooLarge)
        ),
        "MAX_PUSH_PAYLOAD_BYTES is above what the crate accepts"
    );
}

#[test]
fn s4_5_overflowing_body_still_builds_a_deliverable_message() {
    let body = overflowing_notification_body(2862);
    let unfitted = build_push_payload(
        None,
        "Nightly pipeline: GREEN ✅",
        &body,
        Some(uuid::Uuid::new_v4()),
        Some("ci-watch"),
        Some(uuid::Uuid::new_v4()),
        Some(uuid::Uuid::new_v4()),
        &Tap::Modal,
        Some("https://lucidos.test/dev/"),
        Some(7),
    )
    .to_string();
    assert!(
        unfitted.len() > MAX_PUSH_PAYLOAD_BYTES,
        "test body no longer overflows ({} B), pick a longer one",
        unfitted.len()
    );
    assert!(
        matches!(
            build_real_web_push_message(&unfitted),
            Err(web_push::WebPushError::PayloadTooLarge)
        ),
        "the unguarded payload must still be the failure this fix is for"
    );

    let payload = real_world_overflow_payload(&body).to_string();
    assert!(
        payload.len() <= MAX_PUSH_PAYLOAD_BYTES,
        "fitted payload is {} B, over the {} B ceiling",
        payload.len(),
        MAX_PUSH_PAYLOAD_BYTES
    );
    assert!(
        build_real_web_push_message(&payload).is_ok(),
        "the real web_push builder still rejects the fitted payload"
    );
}

#[test]
fn s4_5_truncated_body_is_marked_and_shorter_than_the_original() {
    let body = overflowing_notification_body(2862);
    let payload = real_world_overflow_payload(&body);
    let sent = payload["notification"]["body"].as_str().unwrap();
    assert!(
        sent.len() < body.len(),
        "an overflowing body must be cut, got {} B of {} B",
        sent.len(),
        body.len()
    );
    assert!(
        sent.ends_with(TRUNCATION_MARKER),
        "a cut body must carry the truncation marker, got tail: {:?}",
        &sent[sent.floor_char_boundary(sent.len().saturating_sub(24))..]
    );
    assert!(
        body.starts_with(sent.trim_end_matches(TRUNCATION_MARKER).trim_end()),
        "the kept prefix must be the original body's prefix, unmodified"
    );
}

#[test]
fn s4_5_short_body_passes_through_byte_identical() {
    let nid = uuid::Uuid::new_v4();
    let tid = uuid::Uuid::new_v4();
    let tap = nav_thread(&tid.to_string(), None);
    let body = "Pick one of the three options";
    let unfitted = build_push_payload(
        None,
        "Claude is asking",
        body,
        Some(nid),
        Some("habit-tracker"),
        Some(tid),
        None,
        &tap,
        Some("https://lucidos.test/dev/"),
        Some(2),
    );
    let fitted = build_push_payload_fitted(
        None,
        "Claude is asking",
        body,
        Some(nid),
        Some("habit-tracker"),
        Some(tid),
        None,
        &tap,
        Some("https://lucidos.test/dev/"),
        Some(2),
        None,
    );
    assert_eq!(
        fitted.to_string(),
        unfitted.to_string(),
        "a body that already fits must be passed through unmodified"
    );
    assert_eq!(fitted["notification"]["body"], body);
}

#[test]
fn s4_5_truncation_is_utf8_safe_with_multibyte_content() {
    for pad in 0..64usize {
        let mut body = "✅".repeat(1200);
        body.push_str(&"é".repeat(pad));
        body.push_str(&"✅".repeat(200));
        let payload = build_push_payload_fitted(
            None,
            "Pipeline ✅",
            &body,
            Some(uuid::Uuid::new_v4()),
            None,
            Some(uuid::Uuid::new_v4()),
            None,
            &Tap::Modal,
            Some("https://lucidos.test/dev/"),
            Some(1),
            None,
        );
        let sent = payload["notification"]["body"].as_str().unwrap();
        assert!(
            std::str::from_utf8(sent.as_bytes()).is_ok(),
            "truncated body must stay valid UTF-8"
        );
        let serialized = payload.to_string();
        assert!(
            serialized.len() <= MAX_PUSH_PAYLOAD_BYTES,
            "pad={pad}: fitted payload is {} B, over the ceiling",
            serialized.len()
        );
        assert!(build_real_web_push_message(&serialized).is_ok());
    }
}

#[test]
fn s4_5_truncation_preserves_every_structural_envelope_field() {
    let body = overflowing_notification_body(2862);
    let nid = uuid::Uuid::new_v4();
    let tid = uuid::Uuid::new_v4();
    let eid = uuid::Uuid::new_v4();
    let tap = nav_thread(&tid.to_string(), Some(&eid.to_string()));
    let payload = build_push_payload_fitted(
        None,
        "Nightly pipeline: GREEN ✅",
        &body,
        Some(nid),
        Some("ci-watch"),
        Some(tid),
        Some(eid),
        &tap,
        Some("https://lucidos.test/dev/"),
        Some(7),
        None,
    );

    assert_eq!(payload["web_push"], 8030);
    assert_eq!(payload["app_badge"], 7);
    let notif = &payload["notification"];
    assert_eq!(notif["title"], "Nightly pipeline: GREEN ✅");
    assert_eq!(notif["tag"], nid.to_string());

    let nav_ios = notif["navigate"].as_str().unwrap();
    assert!(nav_ios.starts_with("https://lucidos.test/dev/?"));
    assert!(nav_ios.contains(&format!("notification={nid}")));
    assert!(nav_ios.contains(&format!("thread={tid}")));
    assert!(nav_ios.contains(&format!("event={eid}")));
    assert!(nav_ios.contains("tap="));

    let data = &notif["data"];
    let nav_sw = data["navigate"].as_str().unwrap();
    assert!(nav_sw.starts_with('#'));
    assert_eq!(nav_ios.split_once('?').unwrap().1, &nav_sw[1..]);
    assert_eq!(data["notification_id"], nid.to_string());
    assert_eq!(data["app_id"], "ci-watch");
    assert_eq!(data["thread_id"], tid.to_string());
    assert_eq!(data["event_id"], eid.to_string());
    assert_eq!(data["tap"]["kind"], "navigate");
    assert_eq!(data["tap"]["to"]["id"], tid.to_string());
}

#[test]
fn a_markdown_body_reaches_the_banner_as_plain_text() {
    use crate::scheduler::notifications::Notification;
    let n = Notification {
        id: uuid::Uuid::new_v4(),
        task_id: None,
        app_id: None,
        thread_id: None,
        event_id: None,
        title: "Lucidos is asking".into(),
        message: "**The fix is live.**\n\n- **Weight:** normal\n\nWhat next?".into(),
        read: false,
        created_at: chrono::Utc::now(),
        tap: Tap::Modal,
    };
    let payload = build_wake_payload(None, &n, None, None, None);
    assert_eq!(
        payload["notification"]["body"],
        "The fix is live.\n\n• Weight: normal\n\nWhat next?"
    );
}

#[test]
fn s4_5_wake_payload_of_an_overflowing_body_also_builds() {
    use crate::scheduler::notifications::Notification;
    let tid = uuid::Uuid::new_v4();
    let eid = uuid::Uuid::new_v4();
    let n = Notification {
        id: uuid::Uuid::new_v4(),
        task_id: None,
        app_id: Some("ci-watch".into()),
        thread_id: Some(tid),
        event_id: Some(eid),
        title: "Nightly pipeline: GREEN ✅".into(),
        message: overflowing_notification_body(2862),
        read: false,
        created_at: chrono::Utc::now(),
        tap: nav_thread(&tid.to_string(), Some(&eid.to_string())),
    };
    let payload =
        build_wake_payload(None, &n, Some("https://lucidos.test/dev/"), Some(7), None).to_string();
    assert!(
        payload.len() <= MAX_PUSH_PAYLOAD_BYTES,
        "fitted wake payload is {} B, over the ceiling",
        payload.len()
    );
    assert!(build_real_web_push_message(&payload).is_ok());
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&payload).unwrap()["wake"],
        true
    );
}

#[test]
fn s4_5_wake_budget_is_tighter_than_the_original_send() {
    use crate::scheduler::notifications::Notification;
    let tid = uuid::Uuid::new_v4();
    let body = overflowing_notification_body(2862);
    let n = Notification {
        id: uuid::Uuid::new_v4(),
        task_id: None,
        app_id: None,
        thread_id: Some(tid),
        event_id: None,
        title: "Nightly pipeline: GREEN ✅".into(),
        message: body.clone(),
        read: false,
        created_at: chrono::Utc::now(),
        tap: Tap::Modal,
    };
    let generation = Some(uuid::Uuid::new_v4());
    let wake = build_wake_payload(
        generation,
        &n,
        Some("https://lucidos.test/dev/"),
        Some(7),
        None,
    );
    let send = build_push_payload_fitted(
        generation,
        &n.title,
        &body,
        Some(n.id),
        None,
        Some(tid),
        None,
        &n.tap,
        Some("https://lucidos.test/dev/"),
        Some(7),
        None,
    );
    assert_eq!(
        wake["notification"]["data"]["restore_generation"],
        serde_json::json!(generation)
    );
    assert_eq!(
        send["notification"]["data"]["restore_generation"],
        serde_json::json!(generation)
    );
    let wake_body = wake["notification"]["body"].as_str().unwrap();
    let send_body = send["notification"]["body"].as_str().unwrap();
    assert!(
        wake_body.len() < send_body.len(),
        "the wake must budget for its own `wake: true` flag: wake kept {} B, send kept {} B",
        wake_body.len(),
        send_body.len()
    );
}

#[test]
fn s4_5_escape_heavy_body_keeps_what_actually_fits() {
    for (label, unit, serialized_cost) in [
        ("control chars", "\u{1}", 6usize),
        ("quotes", "\"", 2),
        ("newline-separated lines", "ok\n", 4),
    ] {
        let body = unit.repeat(2862 / unit.len());
        let payload = build_push_payload_fitted(
            None,
            "Nightly pipeline",
            &body,
            Some(uuid::Uuid::new_v4()),
            None,
            None,
            None,
            &Tap::Modal,
            Some("https://lucidos.test/dev/"),
            Some(7),
            None,
        );
        let serialized = payload.to_string();
        assert!(
            serialized.len() <= MAX_PUSH_PAYLOAD_BYTES,
            "{label}: fitted payload is {} B, over the ceiling",
            serialized.len()
        );
        assert!(build_real_web_push_message(&serialized).is_ok());

        let envelope = build_push_payload(
            None,
            "Nightly pipeline",
            "",
            Some(uuid::Uuid::new_v4()),
            None,
            None,
            None,
            &Tap::Modal,
            Some("https://lucidos.test/dev/"),
            Some(7),
        )
        .to_string()
        .len();
        let theoretical =
            (TRUNCATED_PAYLOAD_TARGET_BYTES - envelope) / serialized_cost * unit.len();
        let kept = payload["notification"]["body"]
            .as_str()
            .unwrap()
            .trim_end_matches(TRUNCATION_MARKER)
            .len();
        assert!(
            kept * 4 >= theoretical * 3,
            "{label}: kept only {kept} B of about {theoretical} B that fit"
        );
    }
}

#[test]
fn s4_5_envelope_inside_the_safety_reserve_still_keeps_what_fits() {
    let nid = uuid::Uuid::new_v4();
    let envelope_of = |title: &str| {
        build_push_payload(
            None,
            title,
            "",
            Some(nid),
            None,
            None,
            None,
            &Tap::Modal,
            None,
            Some(1),
        )
        .to_string()
        .len()
    };
    let mut title = String::new();
    while envelope_of(&title) <= TRUNCATED_PAYLOAD_TARGET_BYTES {
        title.push('T');
    }
    let envelope = envelope_of(&title);
    assert!(
        envelope > TRUNCATED_PAYLOAD_TARGET_BYTES && envelope <= MAX_PUSH_PAYLOAD_BYTES,
        "test setup: envelope of {envelope} B is not inside the reserve band"
    );

    let payload = build_push_payload_fitted(
        None,
        &title,
        &overflowing_notification_body(2862),
        Some(nid),
        None,
        None,
        None,
        &Tap::Modal,
        None,
        Some(1),
        None,
    );
    let serialized = payload.to_string();
    assert!(
        serialized.len() <= MAX_PUSH_PAYLOAD_BYTES,
        "fitted payload is {} B, over the ceiling",
        serialized.len()
    );
    assert!(build_real_web_push_message(&serialized).is_ok());
    assert!(
        !payload["notification"]["body"].as_str().unwrap().is_empty(),
        "the reserve must not cost the whole body when the envelope eats into it"
    );
}

#[test]
fn s4_5_envelope_alone_over_the_ceiling_degrades_instead_of_panicking() {
    let title = "T".repeat(MAX_PUSH_PAYLOAD_BYTES + 500);
    let payload = build_push_payload_fitted(
        None,
        &title,
        &overflowing_notification_body(2862),
        Some(uuid::Uuid::new_v4()),
        None,
        None,
        None,
        &Tap::Modal,
        Some("https://lucidos.test/dev/"),
        Some(1),
        None,
    );
    assert_eq!(
        payload["notification"]["body"], "",
        "with no budget left the body must be dropped entirely"
    );
    assert_eq!(payload["notification"]["title"], title);
}

#[test]
fn s4_5_body_fits_exactly_at_the_ceiling_is_not_truncated() {
    let mut body = String::new();
    let build = |b: &str| {
        build_push_payload(
            None,
            "T",
            b,
            None,
            None,
            None,
            None,
            &Tap::Modal,
            None,
            None,
        )
        .to_string()
    };
    while build(&body).len() < MAX_PUSH_PAYLOAD_BYTES {
        body.push('a');
    }
    assert_eq!(build(&body).len(), MAX_PUSH_PAYLOAD_BYTES);
    let fitted = build_push_payload_fitted(
        None,
        "T",
        &body,
        None,
        None,
        None,
        None,
        &Tap::Modal,
        None,
        None,
        None,
    );
    assert_eq!(fitted["notification"]["body"], body);
    assert!(build_real_web_push_message(&fitted.to_string()).is_ok());
}

use crate::engine::presence_tracker::{decide_push_allowed, PresencePong};

use crate::scheduler::PresenceOutcome::{self, AutoRead, NotShown, Shown, Silenced};

fn pong(device_id: &str, outcome: PresenceOutcome) -> PresencePong {
    PresencePong {
        device_id: device_id.into(),
        outcome,
    }
}

#[test]
fn s2_step_a_no_candidates_means_push_allowed() {
    assert!(decide_push_allowed(&[]));
}

#[test]
fn s3_run_presence_check_when_sse_connected_even_with_no_candidate() {
    assert!(
        expected_pong_count(1, 0) > 0,
        "a connected SSE page with a stale heartbeat must still run the PresenceCheck"
    );
}

#[test]
fn s3_skip_presence_check_when_nobody_connected_and_no_candidate() {
    assert_eq!(expected_pong_count(0, 0), 0);
}

#[test]
fn s3_expected_pong_count_is_max_of_both_signals() {
    assert_eq!(expected_pong_count(2, 1), 2);
    assert_eq!(expected_pong_count(1, 3), 3);
}

#[test]
fn s2_step_a_a_handled_pong_means_push_not_allowed() {
    for outcome in [Shown, AutoRead, Silenced] {
        assert!(
            !decide_push_allowed(&[pong("dev-1", outcome)]),
            "{outcome:?}"
        );
    }
}

#[test]
fn s2_step_a_only_not_shown_pongs_means_push_allowed() {
    let pongs = [pong("dev-1", NotShown), pong("dev-2", NotShown)];
    assert!(decide_push_allowed(&pongs));
}

#[test]
fn s2_multi_tab_one_shown_one_hidden_suppresses_push() {
    let pongs = [pong("dev-1", NotShown), pong("dev-1", Shown)];
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
    let ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
              AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 \
              Safari/537.36 Edg/120.0.0.0";
    assert!(is_mac_chromium(ua));
}

#[test]
fn s4_5_ua_predicate_excludes_safari_on_macos() {
    let ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
              AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 \
              Safari/605.1.15";
    assert!(!is_mac_chromium(ua));
}

#[test]
fn s4_5_ua_predicate_excludes_chrome_on_windows() {
    let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
              (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
    assert!(!is_mac_chromium(ua));
}

#[test]
fn s4_5_ua_predicate_excludes_chrome_on_ios() {
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
    let subs = vec![(sub_without_device(), Some(CHROME_MAC_UA.to_string()))];
    let targets = pick_mac_chromium_wake_targets(&subs);
    assert!(targets.is_empty());
}

#[test]
fn s4_5_pick_wake_targets_skips_no_ua() {
    let subs = vec![(sub_with_device("dev-unknown"), None)];
    let targets = pick_mac_chromium_wake_targets(&subs);
    assert!(targets.is_empty());
}

#[test]
fn s4_5_pick_wake_targets_dedupes_multi_tab() {
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
    use crate::engine::presence_tracker::{PresencePongRequest, PresenceTracker};
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
            outcome: Shown,
        });
    });

    let _ =
        tokio::time::timeout(Duration::from_millis(DEADLINE_MS as u64), notify.notified()).await;

    assert!(
        !tracker.decide(nid),
        "DEADLINE_MS={} is too short for a {} ms iOS-cellular pong — \
         engine times out before the page can answer, so push fires \
         on top of the in-app toast",
        DEADLINE_MS,
        SIMULATED_RTT_MS,
    );
}

type TestSend = std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>>;

#[tokio::test(start_paused = true)]
async fn quiet_sends_give_up_at_one_deadline_and_the_healthy_device_still_gets_its_push() {
    let quiet_phone = sub_with_device("quiet-phone");
    let quiet_tablet = sub_with_device("quiet-tablet");
    let healthy = sub_with_device("healthy");
    let deliveries: Vec<(PushSubscription, String)> = [&quiet_phone, &quiet_tablet, &healthy]
        .into_iter()
        .map(|sub| (sub.clone(), "{}".to_string()))
        .collect();
    let mut attempted = Vec::new();
    let started = tokio::time::Instant::now();

    let outcome = tokio::time::timeout(
        PUSH_SEND_TIMEOUT + std::time::Duration::from_secs(1),
        send_each(&deliveries, "notification", |sub, _payload| {
            attempted.push(sub.endpoint.clone());
            let request: TestSend = if sub.endpoint == healthy.endpoint {
                Box::pin(std::future::ready(Ok(())))
            } else {
                Box::pin(std::future::pending())
            };
            Some(request)
        }),
    )
    .await
    .expect("quiet sends must not hold the fan-out past one deadline");

    assert_eq!(
        attempted,
        vec![
            quiet_phone.endpoint,
            quiet_tablet.endpoint,
            healthy.endpoint
        ]
    );
    assert_eq!(
        outcome,
        SendOutcome {
            delivered: 1,
            stale_endpoints: Vec::new(),
        }
    );
    assert_eq!(started.elapsed(), PUSH_SEND_TIMEOUT);
}

#[tokio::test]
async fn a_410_still_marks_the_subscription_stale() {
    let gone = sub_with_device("gone");
    let deliveries = vec![(gone.clone(), "{}".to_string())];

    let outcome = send_each(&deliveries, "notification", |_sub, _payload| {
        let request: TestSend = Box::pin(std::future::ready(Err("410 Gone".to_string())));
        Some(request)
    })
    .await;

    assert_eq!(
        outcome,
        SendOutcome {
            delivered: 0,
            stale_endpoints: vec![gone.endpoint],
        }
    );
}

#[tokio::test(start_paused = true)]
async fn the_fan_out_runs_on_its_own_task_at_the_callers_chain_depth() {
    let _: fn(&SharedEngine, &str, &str, Option<uuid::Uuid>) = send_push_to_all;

    let (depth_tx, depth_rx) = tokio::sync::oneshot::channel();
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        crate::scheduler::user_tasks::EVENT_TRIGGER_DEPTH.scope(3, async {
            spawn_carrying_chain_depth(async move {
                let _ = depth_tx.send(crate::scheduler::user_tasks::current_event_trigger_depth());
                std::future::pending::<()>().await;
            });
        }),
    )
    .await
    .expect("the caller must not wait on a fan-out that never finishes");

    assert_eq!(depth_rx.await.expect("the fan-out task ran"), 3);
}

#[test]
fn the_service_worker_falls_back_to_the_same_tag() {
    let sw = include_str!("../../../lucidos-app/public/sw.js");
    let needle = format!("const DEFAULT_NOTIFICATION_TAG = '{DEFAULT_NOTIFICATION_TAG}';");
    assert!(sw.contains(&needle), "sw.js must carry `{needle}`");
}

#[test]
fn the_service_worker_knows_the_unwatch_action() {
    use crate::scheduler::notifications::UNWATCH_PUSH_ACTION;
    let sw = include_str!("../../../lucidos-app/public/sw.js");
    let needle = format!("const UNWATCH_PUSH_ACTION = '{UNWATCH_PUSH_ACTION}';");
    assert!(sw.contains(&needle), "sw.js must carry `{needle}`");
}

#[test]
fn a_user_only_message_push_groups_per_thread_with_an_unwatch_action() {
    use crate::scheduler::notifications::{Notification, PushGroup, UNWATCH_PUSH_ACTION};
    let thread = uuid::Uuid::new_v4();
    let group = PushGroup::UserOnlyMessages {
        thread_id: thread,
        to_driver: false,
    };
    let push = |nid: uuid::Uuid, body: &str| {
        build_push_payload_fitted(
            None,
            "Sam in \"Launch\"",
            body,
            Some(nid),
            None,
            Some(thread),
            None,
            &Tap::Modal,
            None,
            None,
            Some(&group),
        )
    };
    let first = push(uuid::Uuid::new_v4(), "Kim, can you look?");
    let second = push(uuid::Uuid::new_v4(), &"long ".repeat(2000));
    assert_eq!(first["notification"]["tag"], second["notification"]["tag"]);
    assert_eq!(first["notification"]["tag"], format!("user-{thread}"));
    assert!(second.to_string().len() <= MAX_PUSH_PAYLOAD_BYTES);
    for payload in [&first, &second] {
        let notification = &payload["notification"];
        assert_eq!(notification["actions"][0]["action"], UNWATCH_PUSH_ACTION);
        assert_eq!(
            notification["data"]["unwatch_thread_id"],
            thread.to_string()
        );
    }

    let n = Notification {
        id: uuid::Uuid::new_v4(),
        task_id: None,
        app_id: None,
        thread_id: Some(thread),
        event_id: None,
        title: "Sam".into(),
        message: "Kim, can you look?".into(),
        read: false,
        created_at: chrono::Utc::now(),
        tap: Tap::Modal,
    };
    let wake = build_wake_payload(None, &n, None, None, Some(&group));
    assert_eq!(wake["notification"]["tag"], first["notification"]["tag"]);

    let ordinary = build_wake_payload(None, &n, None, None, None);
    assert_eq!(ordinary["notification"]["tag"], n.id.to_string());
    assert!(ordinary["notification"].get("actions").is_none());

    let to_driver = PushGroup::UserOnlyMessages {
        thread_id: thread,
        to_driver: true,
    };
    let driven = build_wake_payload(None, &n, None, None, Some(&to_driver));
    assert_eq!(driven["notification"]["tag"], first["notification"]["tag"]);
    assert!(driven["notification"].get("actions").is_none());
}

#[test]
fn payload_provenance_survives_fitting() {
    for generation in [None, Some(uuid::Uuid::new_v4())] {
        let payload = build_push_payload_fitted(
            generation,
            "Title",
            &"body".repeat(2000),
            None,
            None,
            None,
            None,
            &Tap::Modal,
            None,
            None,
            None,
        );
        assert_eq!(
            payload["notification"]["data"]["restore_generation"],
            serde_json::json!(generation)
        );
        assert!(payload.to_string().len() <= MAX_PUSH_PAYLOAD_BYTES);
    }
}

#[test]
fn navigate_urls_preserve_push_generation() {
    for generation in [None, Some(uuid::Uuid::new_v4())] {
        let payload = build_push_payload_fitted(
            generation,
            "Title",
            &"body".repeat(2000),
            Some(uuid::Uuid::new_v4()),
            None,
            None,
            None,
            &Tap::Modal,
            None,
            None,
            None,
        );
        let token = generation.map_or_else(|| "none".to_string(), |id| id.to_string());
        for navigate in [
            &payload["notification"]["navigate"],
            &payload["notification"]["data"]["navigate"],
        ] {
            assert!(navigate
                .as_str()
                .unwrap()
                .contains(&format!("restore_generation={token}")));
        }
        assert!(payload.to_string().len() <= MAX_PUSH_PAYLOAD_BYTES);
    }
}

#[tokio::test]
async fn recipient_gateway_totals_reach_normal_and_wake_payloads_without_cross_reader_fallback() {
    use axum::{extract::Query, routing::get, Json, Router};
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    let app = Router::new().route("/~/api/v1/control/unread-total", get(move |Query(q): Query<HashMap<String, String>>| {
        let seen = seen.clone();
        async move {
            let id = q.get("recipient-device-id").expect("every push count selects a recipient");
            seen.lock().unwrap().push(id.clone());
            if id == "slow" {
                tokio::time::sleep(GATEWAY_TOTAL_TIMEOUT * 3).await;
            }
            Json(serde_json::json!({"total": match id.as_str() { "first" => Some(0), "second" => Some(9), _ => None::<i64> }}))
        }
    }));
    let server = lucidos_local_token::test_support::serve_https(app);
    let port = server.port().to_string();
    let mut subs: Vec<_> = ["first", "second", "first", "unknown", "slow"]
        .into_iter()
        .map(|id| {
            let mut sub = sub_with_device(id);
            sub.scope_url = Some("http://gateway.test/dev/".into());
            sub
        })
        .collect();
    let mut missing = sub_without_device();
    missing.scope_url = Some("http://gateway.test/dev/".into());
    subs.push(missing);
    let mut relayed = sub_with_device("relayed");
    relayed.scope_url = Some("http://gateway.test/dev@remote/".into());
    subs.push(relayed);
    subs.push(sub_with_device("direct"));
    let started = std::time::Instant::now();
    let totals = gateway_unread_totals_at(&subs, Some(&port)).await;
    assert!(started.elapsed() < GATEWAY_TOTAL_TIMEOUT * 2);
    let mut seen = requests.lock().unwrap().clone();
    seen.sort();
    assert_eq!(seen, ["first", "second", "slow", "unknown"]);
    assert_eq!(totals["first"], Some(0));
    assert_eq!(totals["second"], Some(9));
    assert_eq!(totals["unknown"], None);
    assert_eq!(totals["slow"], None);
    let own = [
        ((Some("first".into()), BadgeScope::Gateway), Some(1)),
        ((Some("second".into()), BadgeScope::Gateway), Some(2)),
        ((Some("slow".into()), BadgeScope::Gateway), Some(4)),
        ((Some("direct".into()), BadgeScope::Own), Some(5)),
        ((None, BadgeScope::Own), Some(12)),
    ]
    .into_iter()
    .collect();
    let notification = crate::scheduler::notifications::Notification {
        id: uuid::Uuid::new_v4(),
        task_id: None,
        app_id: None,
        thread_id: None,
        event_id: None,
        title: "Hi".into(),
        message: "Body".into(),
        read: false,
        created_at: chrono::Utc::now(),
        tap: Tap::Modal,
    };
    for wake in [false, true] {
        let deliveries = badge_deliveries(subs.clone(), &own, &totals, |sub, badge| {
            if wake {
                build_wake_payload(None, &notification, sub.scope_url.as_deref(), badge, None)
            } else {
                build_push_payload_fitted(
                    None,
                    "Hi",
                    "Body",
                    None,
                    None,
                    None,
                    None,
                    &Tap::Modal,
                    sub.scope_url.as_deref(),
                    badge,
                    None,
                )
            }
        });
        let values: Vec<_> = deliveries
            .iter()
            .map(|(_, body)| {
                serde_json::from_str::<serde_json::Value>(body).unwrap()["app_badge"].clone()
            })
            .collect();
        assert_eq!(
            values,
            vec![
                serde_json::json!(0),
                serde_json::json!(9),
                serde_json::json!(0),
                serde_json::Value::Null,
                serde_json::json!(4),
                serde_json::Value::Null,
                serde_json::Value::Null,
                serde_json::json!(5)
            ]
        );
    }
    assert!(gateway_unread_totals_at(&subs, None).await.is_empty());
    server.abort();
}

#[test]
fn legacy_direct_single_reader_subscription_keeps_its_own_badge() {
    let snapshot = crate::gateway_users::UserSnapshot::default();
    assert!(matches!(
        badge_reader(None, BadgeScope::Own, &snapshot),
        Some(crate::gateway_users::Reader::Team)
    ));
    let own = [((None, BadgeScope::Own), Some(12))].into_iter().collect();
    let deliveries = badge_deliveries(
        vec![sub_without_device()],
        &own,
        &Default::default(),
        |_, badge| serde_json::json!({"app_badge": badge}),
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&deliveries[0].1).unwrap()["app_badge"],
        12
    );
}

#[test]
fn revoked_badge_recipient_cannot_fall_back_to_the_team_on_a_known_gateway() {
    use crate::gateway_users::{User, UserSnapshot};
    let snapshot = UserSnapshot {
        users: vec![
            User {
                id: "user-one".into(),
                name: "Sam".into(),
                emails: vec![],
                device_ids: vec!["first".into()],
            },
            User {
                id: "user-two".into(),
                name: "Alex".into(),
                emails: vec![],
                device_ids: vec!["second".into()],
            },
        ],
        relayed: vec![],
    };
    assert!(badge_reader(Some("revoked"), BadgeScope::Gateway, &snapshot).is_none());
    assert!(badge_reader(None, BadgeScope::Own, &snapshot).is_none());
    assert!(
        matches!(badge_reader(Some("first"), BadgeScope::Gateway, &snapshot), Some(crate::gateway_users::Reader::User { device_id, .. }) if device_id == "first")
    );
}

#[test]
fn single_reader_directory_keeps_direct_counts_separate_from_gateway_fallbacks() {
    use crate::gateway_users::{User, UserSnapshot};
    for snapshot in [
        UserSnapshot::default(),
        UserSnapshot {
            users: vec![User {
                id: "user-one".into(),
                name: "Sam".into(),
                emails: vec![],
                device_ids: vec!["paired".into()],
            }],
            relayed: vec![],
        },
    ] {
        for device in [None, Some("unpaired"), Some("")] {
            assert!(matches!(
                badge_reader(device, BadgeScope::Own, &snapshot),
                Some(crate::gateway_users::Reader::Team)
            ));
            assert!(badge_reader(device, BadgeScope::Gateway, &snapshot).is_none());
            assert!(badge_reader(device, BadgeScope::Relayed, &snapshot).is_none());
            let mut direct = sub_without_device();
            direct.device_id = device.map(str::to_string);
            let mut gateway = direct.clone();
            gateway.scope_url = Some("http://gateway.test/dev/".into());
            let own = [((direct.device_id.clone(), BadgeScope::Own), Some(12))]
                .into_iter()
                .collect();
            let deliveries = badge_deliveries(
                vec![direct, gateway],
                &own,
                &Default::default(),
                |_, badge| serde_json::json!({"app_badge": badge}),
            );
            let values: Vec<_> = deliveries
                .iter()
                .map(|(_, body)| {
                    serde_json::from_str::<serde_json::Value>(body).unwrap()["app_badge"].clone()
                })
                .collect();
            assert_eq!(values, vec![serde_json::json!(12), serde_json::Value::Null]);
        }
    }
}
