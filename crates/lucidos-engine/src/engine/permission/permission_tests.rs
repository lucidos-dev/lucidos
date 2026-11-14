use super::*;
use crate::test_support::{seed_coding_thread, setup_test_db, teardown_test_db};

/// Where these tests keep grant files: nowhere, since no card here is a review card.
const NO_GRANTS_DIR: &str = "/nonexistent/grants";

fn device() -> MessageOrigin {
    MessageOrigin::Device {
        device_id: "phone".into(),
    }
}

fn option(label: &str, choice: PermissionChoice) -> RequestOption {
    RequestOption {
        label: label.into(),
        description: format!("{label}."),
        choice,
    }
}

fn grant(lifetime: Lifetime, holder: Holder) -> PermissionChoice {
    PermissionChoice::grant(lifetime, holder)
}

/// Raise a request for `action` on `thread` offering `options`, and return its
/// card id with the request the card carries.
async fn raise(
    bus: &EventBus,
    thread: Uuid,
    action: Action,
    options: Vec<RequestOption>,
) -> (String, PermissionRequest) {
    let card = RequestCard {
        question: "Allow it?".into(),
        header: "Permission",
        action,
        breadth: None,
        options,
        batch: None,
        unattested_act: None,
    };
    let id = raise_request(bus, thread, card, EventChannel::ClaudeCode)
        .await
        .unwrap();
    let request = card_request(bus.pool(), thread, &id)
        .await
        .unwrap()
        .expect("the card carries its request");
    (id, request)
}

fn option_id(request: &PermissionRequest, wanted: &PermissionChoice) -> String {
    request
        .choices
        .iter()
        .find(|(_, choice)| *choice == wanted)
        .map(|(id, _)| id.clone())
        .expect("the request offers that choice")
}

fn pick(id: String) -> AnswerKind {
    AnswerKind::Selected { option_id: id }
}

fn user(thread: Uuid) -> PermissionUser {
    PermissionUser {
        thread_id: Some(thread),
        trigger_id: None,
    }
}

/// Resolve `answer` as a tap, the way the card's own buttons do.
async fn tap(
    bus: &EventBus,
    thread: Uuid,
    card: &str,
    request: &PermissionRequest,
    answer: &AnswerKind,
    actor: Option<&MessageOrigin>,
) -> Result<Resolution, BoxError> {
    resolve_answer(
        bus,
        NO_GRANTS_DIR.as_ref(),
        thread,
        card,
        request,
        answer,
        actor,
        PickedBy::Tap,
    )
    .await
}

async fn granted_count(pool: &sqlx::PgPool, thread: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type = 'PermissionGranted'",
    )
    .bind(thread)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// A device's pick of a once option grants a permission the gate finds, and
/// the first spend ends it. A second spend does not land.
#[tokio::test]
async fn a_once_grant_covers_one_action() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let once = grant(Lifetime::Once, Holder::Thread { thread_id: thread });
    let (card, request) = raise(
        &bus,
        thread,
        Action::Apply,
        vec![
            option("Allow once", once.clone()),
            option("Deny", PermissionChoice::Refuse),
        ],
    )
    .await;

    let resolution = tap(
        &bus,
        thread,
        &card,
        &request,
        &pick(option_id(&request, &once)),
        Some(&device()),
    )
    .await
    .unwrap();
    assert!(matches!(resolution, Resolution::Granted(_)));

    let permission = live_permission(&pool, &user(thread), Action::Apply, None)
        .await
        .unwrap()
        .expect("the grant is live");
    assert!(spend(&bus, &permission).await);
    assert!(!spend(&bus, &permission).await, "a once grant spends once");
    assert_eq!(
        live_permission(&pool, &user(thread), Action::Apply, None)
            .await
            .unwrap(),
        None
    );
    let ended: Option<String> =
        sqlx::query_scalar("SELECT end_reason FROM permissions WHERE permission_id = $1")
            .bind(permission.permission_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(ended.as_deref(), Some("spent"));
    teardown_test_db(&db).await;
}

/// A revoke that lands before the spend wins: the spend fails, records
/// nothing, and the permission keeps its revoked reason.
#[tokio::test]
async fn a_revoked_grant_never_spends() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let once = grant(Lifetime::Once, Holder::Thread { thread_id: thread });
    let (card, request) = raise(
        &bus,
        thread,
        Action::Apply,
        vec![
            option("Allow once", once.clone()),
            option("Deny", PermissionChoice::Refuse),
        ],
    )
    .await;
    let picked = pick(option_id(&request, &once));
    tap(&bus, thread, &card, &request, &picked, Some(&device()))
        .await
        .unwrap();
    let permission = live_permission(&pool, &user(thread), Action::Apply, None)
        .await
        .unwrap()
        .expect("the grant is live");

    let revoked = revoke(
        &bus,
        NO_GRANTS_DIR.as_ref(),
        permission.permission_id,
        Some(device()),
    )
    .await
    .unwrap();
    assert_eq!(revoked, RevokeOutcome::Revoked);
    assert!(
        !spend(&bus, &permission).await,
        "a revoked grant runs nothing"
    );
    let (ended, spends): (Option<String>, i64) = sqlx::query_as(
        "SELECT end_reason, (SELECT count(*) FROM events WHERE event_type = 'PermissionSpent') \
         FROM permissions WHERE permission_id = $1",
    )
    .bind(permission.permission_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((ended.as_deref(), spends), (Some("revoked"), 0));
    teardown_test_db(&db).await;
}

/// Only a device's pick of a granting option grants. An agent's pick, the
/// refusing option and a typed reply all record nothing.
#[tokio::test]
async fn only_an_owner_pick_of_a_grant_grants() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let standing = grant(Lifetime::Standing, Holder::Workspace);
    let (card, request) = raise(
        &bus,
        thread,
        Action::Archive,
        vec![
            option("Always allow", standing.clone()),
            option("Deny", PermissionChoice::Refuse),
        ],
    )
    .await;
    let agent = MessageOrigin::Agent {
        agent: Default::default(),
    };
    let granting = pick(option_id(&request, &standing));

    let by_agent = tap(&bus, thread, &card, &request, &granting, Some(&agent))
        .await
        .unwrap();
    let by_nobody = tap(&bus, thread, &card, &request, &granting, None)
        .await
        .unwrap();
    let refused = tap(
        &bus,
        thread,
        &card,
        &request,
        &pick(option_id(&request, &PermissionChoice::Refuse)),
        Some(&device()),
    )
    .await
    .unwrap();
    let typed = tap(
        &bus,
        thread,
        &card,
        &request,
        &AnswerKind::FreeText {
            text: "yes, always".into(),
            image_hashes: Vec::new(),
        },
        Some(&device()),
    )
    .await
    .unwrap();
    let unknown = tap(
        &bus,
        thread,
        &card,
        &request,
        &pick("opt-99".into()),
        Some(&device()),
    )
    .await
    .unwrap();

    assert_eq!(by_agent, Resolution::Ignored);
    assert_eq!(by_nobody, Resolution::Ignored);
    assert_eq!(refused, Resolution::Refused);
    assert_eq!(typed, Resolution::Ignored);
    assert_eq!(unknown, Resolution::Ignored);
    assert_eq!(granted_count(&pool, thread).await, 0);
    teardown_test_db(&db).await;
}

/// A permission covers exactly its action, breadth and holder. A lasting
/// grant is used before a once grant beside it, so the once grant survives.
#[tokio::test]
async fn a_permission_covers_exactly_its_scope() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let other = seed_coding_thread(&bus, None).await;

    for lifetime in [Lifetime::Once, Lifetime::ThisThread] {
        let choice = grant(lifetime, Holder::Thread { thread_id: thread });
        let (card, request) = raise(
            &bus,
            thread,
            Action::Apply,
            vec![option("Allow", choice.clone())],
        )
        .await;
        tap(
            &bus,
            thread,
            &card,
            &request,
            &pick(option_id(&request, &choice)),
            Some(&device()),
        )
        .await
        .unwrap();
    }

    let used = live_permission(&pool, &user(thread), Action::Apply, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(used.lifetime, "this_thread");
    assert!(spend(&bus, &used).await, "a lasting grant needs no spend");
    assert!(
        live_permission(&pool, &user(thread), Action::Apply, None)
            .await
            .unwrap()
            .is_some(),
        "both grants are still live"
    );
    assert_eq!(
        live_permission(&pool, &user(other), Action::Apply, None)
            .await
            .unwrap(),
        None,
        "another thread holds nothing"
    );
    assert_eq!(
        live_permission(&pool, &user(thread), Action::Discard, None)
            .await
            .unwrap(),
        None,
        "another action is not covered"
    );
    teardown_test_db(&db).await;
}

/// A breadth narrows a grant: it covers its own breadth only, while a grant
/// with no breadth covers every breadth. A workspace grant reaches every
/// thread, and a trigger grant only that trigger's runs.
#[tokio::test]
async fn breadth_and_holders_narrow_a_grant() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let other = seed_coding_thread(&bus, None).await;

    let card = RequestCard {
        question: "Allow git commands?".into(),
        header: "Permission",
        action: Action::Continue,
        breadth: Some("git".into()),
        options: vec![option(
            "Always allow",
            grant(Lifetime::Standing, Holder::Workspace),
        )],
        batch: None,
        unattested_act: None,
    };
    let id = raise_request(&bus, thread, card, EventChannel::ClaudeCode)
        .await
        .unwrap();
    let request = card_request(&pool, thread, &id).await.unwrap().unwrap();
    let opt = request.choices.keys().next().unwrap().clone();
    tap(&bus, thread, &id, &request, &pick(opt), Some(&device()))
        .await
        .unwrap();

    let trigger_choice = grant(
        Lifetime::Standing,
        Holder::Trigger {
            trigger_id: "nightly".into(),
        },
    );
    let (card, request) = raise(
        &bus,
        thread,
        Action::Cancel,
        vec![option("Allow", trigger_choice.clone())],
    )
    .await;
    tap(
        &bus,
        thread,
        &card,
        &request,
        &pick(option_id(&request, &trigger_choice)),
        Some(&device()),
    )
    .await
    .unwrap();

    let git = live_permission(&pool, &user(other), Action::Continue, Some("git"))
        .await
        .unwrap();
    assert!(git.is_some(), "a workspace grant reaches every thread");
    assert_eq!(
        live_permission(&pool, &user(other), Action::Continue, Some("rm"))
            .await
            .unwrap(),
        None,
        "a breadth covers only itself"
    );
    assert_eq!(
        live_permission(&pool, &user(thread), Action::Cancel, None)
            .await
            .unwrap(),
        None,
        "a trigger grant is not the thread's"
    );
    let nightly = PermissionUser {
        thread_id: Some(other),
        trigger_id: Some("nightly".into()),
    };
    assert!(live_permission(&pool, &nightly, Action::Cancel, None)
        .await
        .unwrap()
        .is_some());
    teardown_test_db(&db).await;
}

/// A revoke ends a standing grant once. Revoking it again, or revoking an
/// unknown id, records nothing.
#[tokio::test]
async fn a_revoke_ends_a_standing_grant() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let standing = grant(Lifetime::Standing, Holder::Workspace);
    let (card, request) = raise(
        &bus,
        thread,
        Action::Archive,
        vec![option("Always allow", standing.clone())],
    )
    .await;
    let Resolution::Granted(id) = tap(
        &bus,
        thread,
        &card,
        &request,
        &pick(option_id(&request, &standing)),
        Some(&device()),
    )
    .await
    .unwrap() else {
        panic!("the pick grants");
    };

    assert_eq!(
        revoke(&bus, NO_GRANTS_DIR.as_ref(), id, Some(device()))
            .await
            .unwrap(),
        RevokeOutcome::Revoked
    );
    assert_eq!(
        revoke(&bus, NO_GRANTS_DIR.as_ref(), id, Some(device()))
            .await
            .unwrap(),
        RevokeOutcome::AlreadyEnded
    );
    assert_eq!(
        revoke(&bus, NO_GRANTS_DIR.as_ref(), Uuid::new_v4(), None)
            .await
            .unwrap(),
        RevokeOutcome::NotFound
    );
    assert_eq!(
        live_permission(&pool, &user(thread), Action::Archive, None)
            .await
            .unwrap(),
        None
    );
    let live = list(&pool, true).await.unwrap();
    let all = list(&pool, false).await.unwrap();
    assert!(live.is_empty());
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].end_reason.as_deref(), Some("revoked"));
    teardown_test_db(&db).await;
}

/// An *until the plan lands* grant ends when its thread's change applies.
#[tokio::test]
async fn a_landed_plan_ends_its_grants() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let until = grant(
        Lifetime::UntilPlanLands,
        Holder::Thread { thread_id: thread },
    );
    let (card, request) = raise(
        &bus,
        thread,
        Action::Apply,
        vec![option("Allow", until.clone())],
    )
    .await;
    tap(
        &bus,
        thread,
        &card,
        &request,
        &pick(option_id(&request, &until)),
        Some(&device()),
    )
    .await
    .unwrap();

    let mut tx = pool.begin().await.unwrap();
    projection::end_until_plan_lands(&mut tx, thread)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let row = &list(&pool, false).await.unwrap()[0];
    assert_eq!(row.end_reason.as_deref(), Some("plan_landed"));
    teardown_test_db(&db).await;
}

/// A spoken pick names its transcript row when it has one. The call talker's
/// pick has none, and a receipt written before either shape still reads.
#[test]
fn a_spoken_pick_names_its_transcript_row_only_when_it_has_one() {
    let row = Uuid::new_v4();
    let judged = PickedBy::Spoken {
        message_event_id: Some(row),
    };
    let talker = PickedBy::Spoken {
        message_event_id: None,
    };
    assert_eq!(
        serde_json::to_value(&judged).unwrap(),
        serde_json::json!({ "kind": "spoken", "message_event_id": row })
    );
    assert_eq!(
        serde_json::to_value(&talker).unwrap(),
        serde_json::json!({ "kind": "spoken" })
    );
    let stored: PickedBy =
        serde_json::from_value(serde_json::json!({ "kind": "spoken", "message_event_id": row }))
            .unwrap();
    assert_eq!(stored, judged);
}
