//! Which device a `navigate_ui` call reaches, and what the agent is told.
//!
//! The incident behind these tests: a turn started on the phone, the user
//! answered the agent's question from the laptop, and the navigate still went
//! to the phone. The tool said "Navigated to file", so the agent told the user
//! it had opened. Nothing opened where they were looking.

use super::{navigate_ui_impl, presence_note, BACKUP_SETTINGS_GUIDE};
use crate::engine::agent_context::{
    build_user_device_preferences_context_for_turn, last_used_device,
};
use crate::engine::event_bus::{BusEvent, EmittedEvent, EventBus};
use crate::engine::thread_events::{
    ActorMode, AnswerKind, EventChannel, EventMeta, MessageOrigin, QuestionOption, ThreadEvent,
};
use crate::test_support::{seed_device, setup_test_db, teardown_test_db};
use serde_json::json;
use sqlx::PgPool;
use tokio::sync::broadcast::Receiver;
use uuid::Uuid;

const PHONE: &str = "device-phone";
const LAPTOP: &str = "device-laptop";

fn device(id: &str) -> MessageOrigin {
    MessageOrigin::Device {
        device_id: id.to_string(),
        label: id.to_string(),
    }
}

fn chat_meta(actor: Option<MessageOrigin>) -> EventMeta {
    EventMeta {
        channel: Some(EventChannel::Chat),
        actor,
        ..EventMeta::NONE
    }
}

async fn setup() -> (EventBus, Receiver<EmittedEvent>, PgPool, String) {
    let (pool, db) = setup_test_db().await;
    seed_device(&pool, PHONE, Some("iPhone Safari/604.1"), Some("My iPhone")).await;
    seed_device(&pool, LAPTOP, Some("Macintosh Chrome"), Some("My MacBook")).await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let events = bus.subscribe();
    (bus, events, pool, db)
}

/// A user message from `from`, the way the chat route writes it. Returns its
/// event id, which is the turn's anchor.
async fn message_from(bus: &EventBus, thread_id: Uuid, from: &str) -> Uuid {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            text: "open the changelog".into(),
            user_image_hashes: vec![],
            device_id: Some(from.to_string()),
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: Some(device(from)),
        },
        meta: chat_meta(None),
    })
    .await
    .expect("MessageReceived emit")
    .expect("MessageReceived persisted")
    .event_id
}

/// The agent asks a question card and the user answers it from `from`.
async fn answer_from(bus: &EventBus, thread_id: Uuid, from: &str) {
    let tool_use_id = format!("toolu-{}", Uuid::new_v4());
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: tool_use_id.clone(),
            cc_session_id: String::new(),
            question: "Which version?".into(),
            options: vec![QuestionOption {
                id: "opt-0".into(),
                label: "Patch".into(),
                description: None,
                preview: None,
            }],
            worktree_path: None,
            multi_select: false,
        },
        meta: chat_meta(None),
    })
    .await
    .expect("UserQuestionAsked emit")
    .expect("UserQuestionAsked persisted");
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAnswered {
            tool_use_id,
            answer: AnswerKind::Selected {
                option_id: "opt-0".into(),
            },
        },
        meta: chat_meta(Some(device(from))),
    })
    .await
    .expect("UserQuestionAnswered emit")
    .expect("UserQuestionAnswered persisted");
}

/// The device actor on the next `NavigationRequested` for `thread_id`, or
/// `None` when it carries no actor (unscoped).
async fn next_navigation_actor(rx: &mut Receiver<EmittedEvent>, thread_id: Uuid) -> Option<String> {
    loop {
        let ev = rx.recv().await.expect("broadcast channel should not close");
        if let BusEvent::Thread {
            thread_id: tid,
            event: ThreadEvent::NavigationRequested { .. },
            meta,
        } = ev.typed
        {
            if tid != thread_id {
                continue;
            }
            return match meta.actor {
                Some(MessageOrigin::Device { device_id, .. }) => Some(device_id),
                _ => None,
            };
        }
    }
}

fn open_changelog() -> serde_json::Value {
    json!({"target": "file", "file_path": "artifacts/releases/changelog.md"})
}

/// The incident, replayed. Before the fix the target stayed on the phone.
#[tokio::test]
async fn a_question_answered_from_another_device_moves_the_target_there() {
    let (bus, mut rx, pool, db) = setup().await;
    let thread_id = Uuid::new_v4();
    let anchor = message_from(&bus, thread_id, PHONE).await;
    answer_from(&bus, thread_id, LAPTOP).await;

    let last_used = last_used_device(&pool, thread_id, Some(anchor), Some(PHONE)).await;
    assert_eq!(last_used.as_deref(), Some(LAPTOP));

    let out = navigate_ui_impl(
        &bus,
        &pool,
        &open_changelog(),
        thread_id,
        last_used.as_deref(),
    )
    .await
    .expect("navigate succeeds");
    assert_eq!(
        next_navigation_actor(&mut rx, thread_id).await.as_deref(),
        Some(LAPTOP)
    );
    assert!(
        out.contains("My MacBook"),
        "names the device by label: {out}"
    );
    assert!(!out.contains("My iPhone"), "names no other device: {out}");

    pool.close().await;
    teardown_test_db(&db).await;
}

/// Only the current turn counts. An answer from the laptop in an EARLIER turn
/// says nothing about where the user is now.
#[tokio::test]
async fn an_answer_from_an_earlier_turn_does_not_move_the_target() {
    let (bus, _rx, pool, db) = setup().await;
    let thread_id = Uuid::new_v4();
    message_from(&bus, thread_id, PHONE).await;
    answer_from(&bus, thread_id, LAPTOP).await;
    let anchor = message_from(&bus, thread_id, PHONE).await;

    let last_used = last_used_device(&pool, thread_id, Some(anchor), Some(PHONE)).await;
    assert_eq!(last_used.as_deref(), Some(PHONE));

    pool.close().await;
    teardown_test_db(&db).await;
}

/// The context block and `navigate_ui` must name the same device. An
/// answer-driven resume carries no device of its own, and the block used to
/// say "unavailable" while the navigate went to the answering device.
#[tokio::test]
async fn the_context_block_names_the_device_navigate_ui_targets() {
    let (bus, _rx, pool, db) = setup().await;
    let thread_id = Uuid::new_v4();
    let anchor = message_from(&bus, thread_id, PHONE).await;
    answer_from(&bus, thread_id, LAPTOP).await;

    let resumed =
        build_user_device_preferences_context_for_turn(&pool, thread_id, Some(anchor), None, None)
            .await;
    assert!(
        resumed.contains(&format!("- id: {LAPTOP}")),
        "a resume names the answering device: {resumed}"
    );

    let other_thread = Uuid::new_v4();
    let anchor = message_from(&bus, other_thread, PHONE).await;
    let fresh = build_user_device_preferences_context_for_turn(
        &pool,
        other_thread,
        Some(anchor),
        Some(PHONE),
        None,
    )
    .await;
    assert!(
        fresh.contains(&format!("- id: {PHONE}")),
        "with no answer the turn's own device stands: {fresh}"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// With no anchor recorded, the resolver keeps the turn's own device.
#[tokio::test]
async fn without_an_anchor_the_turn_device_stands() {
    let (_bus, _rx, pool, db) = setup().await;
    let last_used = last_used_device(&pool, Uuid::new_v4(), None, Some(PHONE)).await;
    assert_eq!(last_used.as_deref(), Some(PHONE));
    pool.close().await;
    teardown_test_db(&db).await;
}

/// An explicit `device` wins over the last used device, and the result names
/// it. This is "open it on my Mac" asked from the phone.
#[tokio::test]
async fn an_explicit_device_gets_the_navigation_and_no_other_does() {
    let (bus, mut rx, pool, db) = setup().await;
    let thread_id = Uuid::new_v4();
    let mut args = open_changelog();
    args["device"] = json!(LAPTOP);

    let out = navigate_ui_impl(&bus, &pool, &args, thread_id, Some(PHONE))
        .await
        .expect("navigate succeeds");
    assert_eq!(
        next_navigation_actor(&mut rx, thread_id).await.as_deref(),
        Some(LAPTOP)
    );
    assert!(out.contains("My MacBook"), "{out}");
    assert!(!out.contains("My iPhone"), "{out}");

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A device nobody has registered would swallow the navigate silently, so the
/// call is refused before anything is emitted.
#[tokio::test]
async fn an_unknown_device_is_refused_and_nothing_is_sent() {
    let (bus, mut rx, pool, db) = setup().await;
    let thread_id = Uuid::new_v4();
    let mut args = open_changelog();
    args["device"] = json!("device-nobody");

    let out = navigate_ui_impl(&bus, &pool, &args, thread_id, Some(PHONE)).await;
    assert!(
        matches!(&out, Err(msg) if msg.contains("device-nobody")),
        "must refuse and name the id: {out:?}"
    );
    assert!(
        matches!(
            rx.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ),
        "nothing may be emitted for a refused device"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A blank `device` is the model filling an optional field, not a choice.
#[tokio::test]
async fn a_blank_device_reads_as_omitted() {
    let (bus, mut rx, pool, db) = setup().await;
    let thread_id = Uuid::new_v4();
    let mut args = open_changelog();
    args["device"] = json!("  ");

    navigate_ui_impl(&bus, &pool, &args, thread_id, Some(PHONE))
        .await
        .expect("a blank device falls back to the last used device");
    assert_eq!(
        next_navigation_actor(&mut rx, thread_id).await.as_deref(),
        Some(PHONE)
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// The result names the thread the event carries, not the alias the model
/// wrote.
#[tokio::test]
async fn a_thread_navigate_names_the_resolved_thread() {
    let (bus, _rx, pool, db) = setup().await;
    let thread_id = Uuid::new_v4();
    let args = json!({"target": "thread", "id": "current"});

    let out = navigate_ui_impl(&bus, &pool, &args, thread_id, Some(PHONE))
        .await
        .expect("navigate succeeds");
    assert!(out.contains(&thread_id.to_string()), "{out}");
    assert!(!out.contains("thread current"), "{out}");

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A trigger or scheduled turn has no device. The navigate stays unscoped, as
/// before, and the result says who it went to.
#[tokio::test]
async fn a_turn_with_no_device_stays_unscoped_and_says_so() {
    let (bus, mut rx, pool, db) = setup().await;
    let thread_id = Uuid::new_v4();

    let out = navigate_ui_impl(&bus, &pool, &open_changelog(), thread_id, None)
        .await
        .expect("navigate succeeds");
    assert_eq!(next_navigation_actor(&mut rx, thread_id).await, None);
    assert!(out.contains("every device"), "{out}");

    pool.close().await;
    teardown_test_db(&db).await;
}

/// The engine never learns whether a page acted, so no result may claim the
/// target opened. The settings guides used to lead with exactly that claim.
#[tokio::test]
async fn no_result_claims_the_target_opened() {
    let (bus, _rx, pool, db) = setup().await;
    let thread_id = Uuid::new_v4();
    for args in [
        open_changelog(),
        json!({"target": "files"}),
        json!({"target": "settings", "settings_view": "models"}),
        json!({"target": "settings", "settings_view": "backup"}),
        json!({"target": "settings", "settings_view": "environment-variables"}),
        json!({"target": "url", "url": "https://example.com"}),
    ] {
        let out = navigate_ui_impl(&bus, &pool, &args, thread_id, Some(PHONE))
            .await
            .expect("navigate succeeds");
        assert!(out.starts_with("Sent"), "{args}: {out}");
        assert!(!out.contains("Navigated"), "{args}: {out}");
        assert!(out.contains("My iPhone"), "{args}: {out}");
    }
    pool.close().await;
    teardown_test_db(&db).await;
}

/// A navigate is transient: a device with Lucidos closed or suspended never
/// gets it. The result must say so, and name the notification as the way to
/// reach that device later, rather than letting "sent" read as delivered.
#[tokio::test]
async fn a_device_not_seen_visible_is_told_as_such_with_a_notification_offer() {
    let (bus, _rx, pool, db) = setup().await;
    let out = navigate_ui_impl(&bus, &pool, &open_changelog(), Uuid::new_v4(), Some(PHONE))
        .await
        .expect("navigate succeeds");
    assert!(
        out.contains("not seen visible on it now"),
        "says the device shows no presence: {out}"
    );
    assert!(out.contains("not stored or retried"), "{out}");
    assert!(
        out.contains("send_notification") && out.contains(r#""kind":"navigate""#),
        "offers the notification tap that reaches the device later: {out}"
    );
    pool.close().await;
    teardown_test_db(&db).await;
}

/// A device with a fresh visible heartbeat most likely got the navigate, so
/// the result carries no warning and no offer.
#[tokio::test]
async fn a_device_seen_visible_carries_no_warning() {
    let (bus, _rx, pool, db) = setup().await;
    crate::core::DevicePresenceStore::record_visible(&pool, PHONE)
        .await
        .unwrap();
    let out = navigate_ui_impl(&bus, &pool, &open_changelog(), Uuid::new_v4(), Some(PHONE))
        .await
        .expect("navigate succeeds");
    assert!(out.contains("Lucidos was visible on it"), "{out}");
    assert!(!out.contains("not stored"), "{out}");
    assert!(!out.contains("send_notification"), "{out}");
    pool.close().await;
    teardown_test_db(&db).await;
}

/// A presence lookup that failed says nothing either way.
#[test]
fn an_unknown_presence_adds_nothing() {
    assert_eq!(presence_note(None, &open_changelog()), "");
}

/// The blurb the agent gets after landing on Settings → System → Backup must
/// name Settings → Accounts as where an account is connected. It must not
/// re-acquire the two claims that rotted: an in-app backup LIST and an in-app
/// RESTORE. Both moved to the workspace picker.
#[test]
fn backup_guide_names_the_accounts_page_and_not_a_restore_ui() {
    let s = BACKUP_SETTINGS_GUIDE;
    assert!(
        s.contains("Settings → Accounts"),
        "must point at the page that actually connects an account: {s}"
    );
    assert!(
        s.contains("workspace picker"),
        "restore lives in the workspace picker and the agent must say so: {s}"
    );
    let lower = s.to_lowercase();
    assert!(
        !lower.contains("restore from an existing"),
        "there is no in-app restore button to advertise: {s}"
    );
    assert!(
        !lower.contains("list of available cloud backups"),
        "the page shows no backup list: {s}"
    );
}
