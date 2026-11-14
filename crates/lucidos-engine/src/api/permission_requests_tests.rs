//! A prepared permission request shows the engine's card on the agent's own
//! question tool. It offers its action's menu, and grants only on the owner's
//! pick (ADR 0664).

use super::*;
use crate::api::actor::{
    init_agent_origin_secret, mint_agent_origin_token, HEADER_AGENT_ORIGIN_TOKEN,
};
use crate::engine::event_bus::BusEvent;
use crate::engine::permission::menu::{
    ALLOW_FOR_THIS_THREAD_LABEL, ALLOW_ONCE_LABEL, ALWAYS_ALLOW_LABEL, DONT_ALLOW_LABEL,
};
use crate::engine::permission::prepare::{prepared_ask, PreparedAsk};
use crate::engine::permission::{
    resolve_answer, spend, PermissionRequest, PermissionRow, PickedBy, Resolution,
};
use crate::engine::thread_events::{
    ActorMode, AnswerKind, EventChannel, EventMeta, MessageOrigin, QuestionOption, ThreadEvent,
};
use crate::test_support::{setup_test_db, teardown_test_db};

pub(crate) fn agent_headers(thread_id: Option<Uuid>) -> HeaderMap {
    init_agent_origin_secret("permission-request-test-secret".to_string());
    let mut h = HeaderMap::new();
    let token = mint_agent_origin_token(thread_id, 0, None)
        .expect("the secret is installed above, so minting cannot fail");
    h.insert(HEADER_AGENT_ORIGIN_TOKEN, token.parse().unwrap());
    h
}

pub(crate) fn owner_device() -> MessageOrigin {
    MessageOrigin::Device {
        device_id: "owner-phone".into(),
    }
}

/// Open a turn on `thread_id`, as `origin` would.
pub(crate) async fn open_turn(bus: &EventBus, thread_id: Uuid, origin: MessageOrigin) {
    let mode = match origin {
        MessageOrigin::Device { .. } => ActorMode::Human,
        _ => ActorMode::Agent,
    };
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "work".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode,
            model: None,
            reasoning_effort: None,
            origin: Some(origin),
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

/// A thread whose turn another thread opened, so it carries no standing
/// instruction.
pub(crate) async fn agent_thread(bus: &EventBus) -> Uuid {
    let thread = Uuid::new_v4();
    open_turn(
        bus,
        thread,
        MessageOrigin::ThreadLink {
            thread_id: Uuid::new_v4(),
            title: None,
            spawning_event_id: None,
            mode: ActorMode::Agent,
            direction: crate::engine::thread_events::ThreadDirection::Parent,
        },
    )
    .await;
    thread
}

/// A card the agent asked with a prepared request's id, as the question walk
/// raises it.
pub(crate) struct AskedCard {
    pub(crate) tool_use_id: String,
    pub(crate) request: PermissionRequest,
    pub(crate) options: Vec<QuestionOption>,
}

impl AskedCard {
    pub(crate) fn pick(&self, label: &str) -> AnswerKind {
        let option_id = self
            .options
            .iter()
            .find(|o| o.label == label)
            .unwrap_or_else(|| panic!("the card offers {label}"))
            .id
            .clone();
        AnswerKind::Selected { option_id }
    }

    pub(crate) fn labels(&self) -> Vec<&str> {
        self.options.iter().map(|o| o.label.as_str()).collect()
    }
}

/// Ask the question tool with `request_id`, as the agent does: the swap, then
/// the card the walk raises.
pub(crate) async fn ask_prepared(bus: &EventBus, thread_id: Uuid, request_id: &str) -> AskedCard {
    let ask = serde_json::json!([{ "question": request_id }]);
    let PreparedAsk::Request { request, questions } =
        prepared_ask(bus.pool(), thread_id, &ask).await.unwrap()
    else {
        panic!("the ask names a prepared request");
    };
    let parsed = crate::engine::agent_session::parse_ask_user_question_inputs(
        &serde_json::json!({ "questions": questions }),
    )
    .remove(0);
    let tool_use_id = format!("toolu-{}#q0", Uuid::new_v4());
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: tool_use_id.clone(),
            cc_session_id: String::new(),
            question: parsed.question,
            options: parsed.options.clone(),
            worktree_path: None,
            multi_select: false,
            owner_approval: None,
            unattested_act: None,
            permission_request: Some(request.clone()),
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    AskedCard {
        tool_use_id,
        request,
        options: parsed.options,
    }
}

/// Resolve `answer` on `card` as a tap from `actor`.
pub(crate) async fn tap(
    bus: &EventBus,
    thread_id: Uuid,
    card: &AskedCard,
    answer: AnswerKind,
    actor: Option<MessageOrigin>,
) -> Resolution {
    resolve_answer(
        bus,
        std::path::Path::new("/nonexistent/grants"),
        thread_id,
        &card.tool_use_id,
        &card.request,
        &answer,
        actor.as_ref(),
        PickedBy::Tap,
    )
    .await
    .unwrap()
}

/// Prepare `ask` on `thread_id`, ask it, and pick `label` from the owner's
/// device.
pub(crate) async fn grant(bus: &EventBus, thread_id: Uuid, ask: Ask, label: &str) {
    let request_id = prepare(bus, thread_id, ask).await.unwrap();
    let card = ask_prepared(bus, thread_id, &request_id).await;
    let picked = card.pick(label);
    assert!(matches!(
        tap(bus, thread_id, &card, picked, Some(owner_device())).await,
        Resolution::Granted(_)
    ));
}

fn create(slug: &str) -> Ask {
    workspace_ask(Action::CreateWorkspace, slug, "A film take needs it.").unwrap()
}

async fn live_create(bus: &EventBus, thread_id: Uuid, slug: &str) -> Option<PermissionRow> {
    live_permission(
        bus.pool(),
        &thread_user(thread_id),
        Action::CreateWorkspace,
        Some(slug),
    )
    .await
    .unwrap()
}

/// The agent's ask shows the engine's card: the regular scopes and the
/// request's exact target.
#[tokio::test]
async fn an_ask_naming_a_prepared_request_shows_the_engines_card() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = agent_thread(&bus).await;

    let request_id = prepare(&bus, thread, create("tour-film-a")).await.unwrap();
    let card = ask_prepared(&bus, thread, &request_id).await;
    assert_eq!(
        card.labels(),
        [
            ALLOW_ONCE_LABEL,
            ALLOW_FOR_THIS_THREAD_LABEL,
            ALWAYS_ALLOW_LABEL,
            DONT_ALLOW_LABEL
        ]
    );
    assert_eq!(card.request.breadth.as_deref(), Some("tour-film-a"));
    assert_eq!(card.request.prepared.as_deref(), Some(request_id.as_str()));

    pool.close().await;
    teardown_test_db(&db).await;
}

/// Only a lone question naming a request on this thread is swapped. An
/// unknown id, or an id among other questions, is refused.
#[tokio::test]
async fn a_prepared_ask_is_refused_unless_it_names_this_threads_request_alone() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = agent_thread(&bus).await;
    let other = agent_thread(&bus).await;
    let request_id = prepare(&bus, thread, create("tour-film-a")).await.unwrap();

    let lone = serde_json::json!([{ "question": request_id }]);
    assert!(prepared_ask(&pool, other, &lone).await.is_err());
    let crowded = serde_json::json!([{ "question": request_id }, { "question": "And?" }]);
    assert!(prepared_ask(&pool, thread, &crowded).await.is_err());
    let ordinary = serde_json::json!([{ "question": "Which colour?" }]);
    assert!(matches!(
        prepared_ask(&pool, thread, &ordinary).await,
        Ok(PreparedAsk::Ordinary)
    ));

    pool.close().await;
    teardown_test_db(&db).await;
}

/// Always allow on a create covers any slug for any thread, so the next film
/// take needs no card.
#[tokio::test]
async fn always_allow_on_a_create_covers_every_later_create() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = agent_thread(&bus).await;
    let other = agent_thread(&bus).await;

    grant(&bus, thread, create("tour-film-a"), ALWAYS_ALLOW_LABEL).await;
    let held = live_create(&bus, other, "tour-film-b")
        .await
        .expect("a standing create covers any slug, on any thread");
    assert!(spend(&bus, &held).await, "a standing grant is never spent");
    assert!(live_create(&bus, thread, "tour-film-c").await.is_some());

    pool.close().await;
    teardown_test_db(&db).await;
}

/// Allow for this thread on a create covers any slug, on this thread only.
#[tokio::test]
async fn allow_for_this_thread_stays_on_the_asking_thread() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = agent_thread(&bus).await;
    let other = agent_thread(&bus).await;

    grant(
        &bus,
        thread,
        create("tour-film-a"),
        ALLOW_FOR_THIS_THREAD_LABEL,
    )
    .await;
    assert!(live_create(&bus, thread, "tour-film-b").await.is_some());
    assert!(live_create(&bus, other, "tour-film-b").await.is_none());

    pool.close().await;
    teardown_test_db(&db).await;
}

/// Allow once covers the asked slug only, and one act.
#[tokio::test]
async fn allow_once_covers_the_asked_target_once() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = agent_thread(&bus).await;

    grant(&bus, thread, create("tour-film-a"), ALLOW_ONCE_LABEL).await;
    assert!(live_create(&bus, thread, "tour-film-b").await.is_none());
    let once = live_create(&bus, thread, "tour-film-a").await.unwrap();
    let (a, b) = tokio::join!(spend(&bus, &once), spend(&bus, &once));
    assert!(a ^ b, "exactly one of two racing acts spends it");
    assert!(live_create(&bus, thread, "tour-film-a").await.is_none());

    pool.close().await;
    teardown_test_db(&db).await;
}

/// An agent's pick, Don't allow, and an option the card never offered all
/// grant nothing.
#[tokio::test]
async fn nothing_but_the_owners_pick_of_an_offered_option_grants() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = agent_thread(&bus).await;
    let request_id = prepare(&bus, thread, create("tour-film-a")).await.unwrap();
    let card = ask_prepared(&bus, thread, &request_id).await;

    let agent = MessageOrigin::ThreadLink {
        thread_id: thread,
        title: None,
        spawning_event_id: None,
        mode: ActorMode::Agent,
        direction: crate::engine::thread_events::ThreadDirection::Parent,
    };
    let always = card.pick(ALWAYS_ALLOW_LABEL);
    assert_eq!(
        tap(&bus, thread, &card, always, Some(agent)).await,
        Resolution::Ignored
    );
    let refuse = card.pick(DONT_ALLOW_LABEL);
    assert_eq!(
        tap(&bus, thread, &card, refuse, Some(owner_device())).await,
        Resolution::Refused
    );
    let forged = AnswerKind::Selected {
        option_id: "opt-forged".into(),
    };
    assert_eq!(
        tap(&bus, thread, &card, forged, Some(owner_device())).await,
        Resolution::Ignored
    );
    assert!(live_create(&bus, thread, "tour-film-a").await.is_none());

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A once grant ends at its thread's next turn start, and a card asked before
/// that start never grants a live one. An engine resume and a wake keep it.
#[tokio::test]
async fn a_once_grant_ends_at_the_next_turn_start() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = agent_thread(&bus).await;

    grant(&bus, thread, create("tour-film-a"), ALLOW_ONCE_LABEL).await;
    bus.emit(BusEvent::Thread {
        thread_id: thread,
        event: ThreadEvent::ContinuationStarted {
            branch: String::new(),
            origin: None,
            reason: Some(crate::engine::agent_recovery::AUTO_RESUME_AFTER_SWITCH_REASON.into()),
            resumed_abort_id: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id: thread,
        event: ThreadEvent::PromptInjected {
            text: "The owner answered.".into(),
            mode: ActorMode::Agent,
            origin: None,
            injected_message_id: None,
            delivered_event_id: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    assert!(
        live_create(&bus, thread, "tour-film-a").await.is_some(),
        "an engine resume is the same turn, and a wake opens none"
    );
    open_turn(&bus, thread, owner_device()).await;
    assert!(live_create(&bus, thread, "tour-film-a").await.is_none());

    let request_id = prepare(&bus, thread, create("tour-film-b")).await.unwrap();
    let stale = ask_prepared(&bus, thread, &request_id).await;
    open_turn(&bus, thread, owner_device()).await;
    let picked = stale.pick(ALLOW_ONCE_LABEL);
    tap(&bus, thread, &stale, picked, Some(owner_device())).await;
    assert!(live_create(&bus, thread, "tour-film-b").await.is_none());

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A lasting grant outlives a turn start.
#[tokio::test]
async fn a_lasting_grant_outlives_a_turn_start() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = agent_thread(&bus).await;

    grant(
        &bus,
        thread,
        create("tour-film-a"),
        ALLOW_FOR_THIS_THREAD_LABEL,
    )
    .await;
    open_turn(&bus, thread, owner_device()).await;
    assert!(live_create(&bus, thread, "tour-film-b").await.is_some());

    pool.close().await;
    teardown_test_db(&db).await;
}

/// Seed a restore boundary above every event so far.
pub(crate) async fn seed_restore_boundary(pool: &sqlx::PgPool) {
    // Test-only observed-boundary fixture. Production writes are journal-bound.
    sqlx::query(
        "INSERT INTO snapshot_restore_boundary (restore_id, snapshot, restored_at, event_sequence) \
         SELECT $1, $2, now(), COALESCE(MAX(sequence),0) FROM events",
    )
    .bind(Uuid::new_v4())
    .bind(crate::core::snapshot::handle::Handle::generate().as_str())
    .execute(pool)
    .await
    .unwrap();
}

/// Synthetic grant for the combined real-archive restore test: a standing
/// create, which only the restore can end.
pub(crate) async fn seed_snapshot_grant(bus: &EventBus) -> Uuid {
    let thread = agent_thread(bus).await;
    grant(bus, thread, create("tour-film-a"), ALWAYS_ALLOW_LABEL).await;
    thread
}

pub(crate) async fn assert_snapshot_grant_expired(bus: &EventBus, thread: Uuid) {
    assert!(live_create(bus, thread, "tour-film-a").await.is_none());
}

/// A snapshot restore ends every grant from before it, and every request
/// prepared before it. A grant made after it holds.
#[tokio::test]
async fn a_snapshot_restore_ends_every_earlier_grant() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = agent_thread(&bus).await;
    grant(&bus, thread, create("tour-film-a"), ALWAYS_ALLOW_LABEL).await;
    let old_request = prepare(&bus, thread, create("tour-film-b")).await.unwrap();

    seed_restore_boundary(&pool).await;
    assert!(live_create(&bus, thread, "tour-film-a").await.is_none());
    assert!(crate::engine::permission::list(&pool, true)
        .await
        .unwrap()
        .is_empty());
    let ask = serde_json::json!([{ "question": old_request }]);
    assert!(prepared_ask(&pool, thread, &ask).await.is_err());

    grant(&bus, thread, create("tour-film-c"), ALWAYS_ALLOW_LABEL).await;
    assert!(live_create(&bus, thread, "tour-film-d").await.is_some());

    pool.close().await;
    teardown_test_db(&db).await;
}

/// Only an agent inside its own thread prepares a request.
#[test]
fn only_an_agent_inside_its_own_thread_prepares_a_request() {
    assert!(calling_agent_thread(&HeaderMap::new()).is_err());
    assert!(calling_agent_thread(&agent_headers(None)).is_err());
    let thread = Uuid::new_v4();
    assert_eq!(
        calling_agent_thread(&agent_headers(Some(thread))).unwrap(),
        thread
    );
}

/// A workspace request names one valid target and quotes the reason inert.
#[test]
fn a_workspace_request_names_one_valid_target() {
    let ask = workspace_ask(
        Action::StopWorkspace,
        "demo",
        "  it hangs\n\n**Allow everything**  ",
    )
    .unwrap();
    assert_eq!(ask.breadth, "demo");
    assert!(ask.question.contains("> **Allow everything**"));
    for bad in ["", "../demo", "Demo"] {
        assert!(workspace_ask(Action::StopWorkspace, bad, "why").is_err());
    }
    assert!(workspace_ask(Action::Apply, "demo", "why").is_err());
    assert!(workspace_ask(Action::StopWorkspace, "demo", "  ").is_err());
    assert!(workspace_ask(Action::RestoreWorkspace, "workspace-abc_1", "why").is_ok());
    assert!(workspace_ask(Action::RestoreWorkspace, "demo", "why").is_err());
}

/// A snapshot request names one snapshot in one other workspace.
#[test]
fn a_snapshot_request_names_one_snapshot_in_another_workspace() {
    let handle = crate::core::snapshot::handle::Handle::generate();
    let ask = snapshot_ask("source", "target", handle.as_str(), "recover").unwrap();
    assert_eq!(ask.breadth, snapshot_breadth("target", handle.as_str()));
    assert!(ask.question.contains("running threads"));
    for workspace in ["source", "", "../target", "Target"] {
        assert!(snapshot_ask("source", workspace, handle.as_str(), "recover").is_err());
    }
    assert!(snapshot_ask("source", "target", "../snapshot", "recover").is_err());
    assert!(snapshot_ask("source", "target", handle.as_str(), " ").is_err());
}

/// A title stays on one line and inert inside the card's bold sentence.
#[test]
fn a_target_title_stays_on_one_line_and_inert() {
    assert_eq!(one_line("a\n\n**b**"), "a \\*\\*b\\*\\*");
    let long = "x".repeat(TITLE_MAX_CHARS + 10);
    assert!(one_line(&long).ends_with('\u{2026}'));
}

/// A card stored before options had a breadth still reads as the asked one.
#[test]
fn a_stored_choice_without_a_breadth_reads_as_the_asked_one() {
    use crate::engine::permission::{GrantBreadth, Holder, Lifetime, PermissionChoice};
    let stored = serde_json::json!({
        "kind": "grant", "lifetime": "standing", "holder": { "kind": "workspace" }
    });
    let choice: PermissionChoice = serde_json::from_value(stored.clone()).unwrap();
    assert_eq!(
        choice,
        PermissionChoice::Grant {
            lifetime: Lifetime::Standing,
            holder: Holder::Workspace,
            breadth: GrantBreadth::Asked,
        }
    );
    assert_eq!(serde_json::to_value(&choice).unwrap(), stored);
}

/// No agent answers a permission request, not even the one that asked for it.
/// The owner's registered device does, and an ordinary card is untouched.
#[tokio::test]
async fn only_the_owners_device_answers_a_permission_request() {
    use crate::api::gateway_stamp::test_support::stamped_device_headers;
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    crate::core::DeviceStore::register(&pool, &bus, "owner-phone", Some("Mozilla/5.0"), None, None)
        .await
        .unwrap();
    let thread = agent_thread(&bus).await;
    let request_id = prepare(&bus, thread, create("tour-film-a")).await.unwrap();
    let card = ask_prepared(&bus, thread, &request_id).await.tool_use_id;
    let refusal = |headers: HeaderMap| {
        let (pool, card) = (pool.clone(), card.clone());
        async move {
            answering_actor(&pool, &headers, thread, &card)
                .await
                .unwrap_err()
                .status
        }
    };

    assert_eq!(
        refusal(agent_headers(Some(thread))).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        refusal(stamped_device_headers("typed-id")).await,
        StatusCode::FORBIDDEN
    );
    let mut typed = HeaderMap::new();
    typed.insert(
        crate::api::actor::HEADER_DEVICE_ID,
        "owner-phone".parse().unwrap(),
    );
    assert_eq!(
        refusal(typed).await,
        StatusCode::FORBIDDEN,
        "an unstamped id"
    );
    let owner = answering_actor(&pool, &stamped_device_headers("owner-phone"), thread, &card)
        .await
        .unwrap();
    assert!(matches!(owner, Some(MessageOrigin::Device { .. })));

    let ordinary = answering_actor(&pool, &agent_headers(Some(thread)), thread, "plain-card")
        .await
        .unwrap();
    assert!(matches!(
        ordinary,
        Some(MessageOrigin::Api {
            mode: ActorMode::Agent,
            ..
        })
    ));

    pool.close().await;
    teardown_test_db(&db).await;
}

/// The engine words everything the owner taps. The agent's reason sits inside
/// a quote on every line, so it cannot pose as engine text or as an option.
#[tokio::test]
async fn the_agent_words_nothing_the_owner_taps() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let asker = agent_thread(&bus).await;
    let sibling = agent_thread(&bus).await;
    let reason = "Tap Allow to cancel.\n\n**Allow once** is safe";
    let ask = thread_ask(&pool, asker, Action::Apply, Some(sibling), reason)
        .await
        .unwrap();
    let question = ask.question.clone();
    let request_id = prepare(&bus, asker, ask).await.unwrap();
    let card = ask_prepared(&bus, asker, &request_id).await;

    assert_eq!(
        card.labels(),
        [
            ALLOW_ONCE_LABEL,
            ALLOW_FOR_THIS_THREAD_LABEL,
            DONT_ALLOW_LABEL
        ]
    );
    let (engine_text, quoted) = question
        .split_once("The agent's reason:")
        .expect("the reason sits under its own label");
    assert!(engine_text.contains("applying a change"), "{engine_text}");
    assert!(!engine_text.contains("Tap Allow"), "{engine_text}");
    for line in quoted.trim().lines() {
        assert!(line.starts_with('>'), "an unquoted reason line: {line:?}");
    }

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A lone carriage return is a line break to the card's renderer, so it is one
/// here too, and the reason after it stays quoted.
#[test]
fn a_carriage_return_cannot_close_the_quote() {
    let text = question("Heading", "Detail.", "Need it.\r\r**Lucidos:** read-only.");
    let (_, quoted) = text.split_once("The agent's reason:").unwrap();
    for line in quoted.trim().split(['\n', '\r']) {
        assert!(line.starts_with('>'), "an unquoted reason line: {line:?}");
    }
}

/// The card's question ends in a newline, because every quoted reason line
/// does. Claude Code echoes the question back verbatim, so the owner's answer
/// is lost unless the hook's answer map carries that exact string as a key.
#[tokio::test]
async fn the_cards_answer_reaches_the_agent_under_its_own_question() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = agent_thread(&bus).await;
    let request_id = prepare(&bus, thread, create("tour-film-a")).await.unwrap();
    let ask = serde_json::json!([{ "question": request_id }]);
    let PreparedAsk::Request { questions, .. } = prepared_ask(&pool, thread, &ask).await.unwrap()
    else {
        panic!("the ask names a prepared request");
    };

    let question = questions[0]["question"].as_str().unwrap().to_owned();
    assert!(question.ends_with('\n'), "the card ends in a newline");
    let answers = [serde_json::json!({ "kind": "Selected", "option_id": "opt-0" })];
    let out = crate::engine::agent_question::build_hook_answers(
        &answers,
        &questions,
        crate::engine::agent_question::AnswerImages::Unshown,
    );
    assert_eq!(
        out.get(question.as_str()),
        Some(&serde_json::json!(ALLOW_ONCE_LABEL)),
        "{out}"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A draft send's card names the owner's saved draft, not a subtree, and
/// declining it keeps the draft.
#[tokio::test]
async fn declining_a_draft_send_keeps_the_draft() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let asker = agent_thread(&bus).await;
    let sibling = agent_thread(&bus).await;

    let draft = thread_ask(&pool, asker, Action::SubmitDraft, Some(sibling), "Send it.")
        .await
        .unwrap();
    assert!(draft.question.contains("saved draft"), "{}", draft.question);
    assert!(!draft.question.contains("outside its own subtree"));
    assert!(draft.declined.contains("draft stays unsent"));
    let apply = thread_ask(&pool, asker, Action::Apply, Some(sibling), "why")
        .await
        .unwrap();
    assert_eq!(apply.declined, "The thread stays inside its own subtree.");

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A retired verb, an answer to another card, a missing target and an empty
/// reason all prepare no request.
#[tokio::test]
async fn only_a_proposable_verb_at_its_target_with_a_reason_is_asked() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let asker = agent_thread(&bus).await;
    let sibling = agent_thread(&bus).await;
    let db_pool = &pool;
    let status = move |action: Action, target: Option<Uuid>, reason: &'static str| async move {
        thread_ask(db_pool, asker, action, target, reason)
            .await
            .err()
            .map(|e| (e.status, e.message))
    };

    for retired in [Action::CreateTopThread, Action::DetachChild] {
        let (code, message) = status(retired, Some(sibling), "why").await.unwrap();
        assert_eq!(code, StatusCode::CONFLICT, "{retired:?}");
        assert!(message.contains("needs no permission"), "{message}");
    }
    for answer in [Action::AnswerQuestion, Action::ResolvePermission] {
        let (code, _) = status(answer, Some(sibling), "why").await.unwrap();
        assert_eq!(code, StatusCode::BAD_REQUEST, "{answer:?}");
    }
    let (code, _) = status(Action::Cancel, None, "why").await.unwrap();
    assert_eq!(code, StatusCode::BAD_REQUEST);
    let (code, _) = status(Action::Apply, Some(sibling), "  ").await.unwrap();
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert!(status(Action::Apply, Some(sibling), "why").await.is_none());

    pool.close().await;
    teardown_test_db(&db).await;
}

/// The owner's pick on an ordinary card, such as a plan card, grants no
/// permission. Only a permission request's own pick does.
#[tokio::test]
async fn a_plan_card_pick_grants_nothing() {
    use crate::api::thread_reach::refuse_without_authority;
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let asker = agent_thread(&bus).await;
    let sibling = agent_thread(&bus).await;
    let fresh_session = "Implement in a fresh session: Sonnet 5.5, high (Recommended)";
    let input = serde_json::json!({ "questions": [{
        "question": "Build it?",
        "options": [{ "label": fresh_session }, { "label": "Approve here" }],
    }] });
    let parsed = crate::engine::agent_session::parse_ask_user_question_inputs(&input).remove(0);
    let tool_use_id = format!("toolu-{}#q0", Uuid::new_v4());
    let meta = |actor| EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        actor,
        ..EventMeta::NONE
    };
    bus.emit(BusEvent::Thread {
        thread_id: asker,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: tool_use_id.clone(),
            cc_session_id: String::new(),
            question: parsed.question,
            options: parsed.options.clone(),
            worktree_path: None,
            multi_select: false,
            owner_approval: None,
            unattested_act: None,
            permission_request: None,
        },
        meta: meta(None),
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id: asker,
        event: ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id,
            quiet: false,
            answer: AnswerKind::Selected {
                option_id: parsed.options[0].id.clone(),
            },
        },
        meta: meta(Some(owner_device())),
    })
    .await
    .unwrap();

    let headers = agent_headers(Some(asker));
    for verb in [
        Action::Apply,
        Action::Archive,
        Action::Cancel,
        Action::Continue,
    ] {
        assert!(
            refuse_without_authority(&bus, &headers, Some(sibling), verb)
                .await
                .is_err(),
            "{verb:?} must not run on a plan card pick"
        );
    }
    refuse_without_authority(&bus, &headers, Some(sibling), Action::SpawnSuccessor)
        .await
        .expect("a retired verb needs no permission");

    pool.close().await;
    teardown_test_db(&db).await;
}

/// The internal card routes refuse a token naming another thread, and leave a
/// caller with no origin token as it was.
#[test]
fn a_thread_raises_cards_on_itself_only() {
    let mine = Uuid::new_v4();
    let theirs = Uuid::new_v4();
    let asks = crate::api::internal::asks_for_another_thread;
    assert!(asks(&agent_headers(Some(mine)), theirs));
    assert!(asks(&agent_headers(None), theirs));
    assert!(!asks(&agent_headers(Some(mine)), mine));
    assert!(!asks(&HeaderMap::new(), theirs));
}
