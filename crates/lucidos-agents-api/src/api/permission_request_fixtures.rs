use super::*;
use crate::engine::event_bus::BusEvent;
#[cfg(feature = "test-support")]
use crate::engine::permission::menu::ALWAYS_ALLOW_LABEL;
use crate::engine::permission::prepare::{prepared_ask, PreparedAsk};
use crate::engine::permission::{
    resolve_answer, PermissionRequest, PermissionRow, PickedBy, Resolution,
};
use crate::engine::thread_events::{
    ActorMode, AnswerKind, EventChannel, EventMeta, QuestionOption, ThreadEvent,
};

pub(crate) fn owner_device() -> MessageOrigin {
    MessageOrigin::Device {
        device_id: "owner-phone".into(),
    }
}

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

    #[cfg(test)]
    pub(crate) fn labels(&self) -> Vec<&str> {
        self.options.iter().map(|o| o.label.as_str()).collect()
    }
}

pub(crate) async fn ask_prepared(bus: &EventBus, thread_id: Uuid, request_id: &str) -> AskedCard {
    let ask = serde_json::json!([{ "question": request_id }]);
    let PreparedAsk::Request { request, questions } =
        prepared_ask(bus.pool(), thread_id, &ask).await.unwrap()
    else {
        panic!("the ask names a prepared request");
    };
    let parsed = crate::engine::question_parsing::parse_ask_user_question_inputs(
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

pub(crate) async fn tap(
    bus: &EventBus,
    thread_id: Uuid,
    card: &AskedCard,
    answer: AnswerKind,
    actor: Option<MessageOrigin>,
) -> Resolution {
    resolve_answer(
        bus,
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

pub(crate) async fn answer_and_tap(
    bus: &EventBus,
    thread_id: Uuid,
    card: &AskedCard,
    answer: AnswerKind,
) -> Resolution {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: card.tool_use_id.clone(),
            answer: answer.clone(),
            quiet: true,
        },
        meta: EventMeta::with_actor(Some(owner_device())),
    })
    .await
    .unwrap();
    tap(bus, thread_id, card, answer, Some(owner_device())).await
}

pub(crate) async fn grant(bus: &EventBus, thread_id: Uuid, ask: Ask, label: &str) {
    let request_id = prepare(bus, thread_id, ask).await.unwrap();
    let card = ask_prepared(bus, thread_id, &request_id).await;
    let picked = card.pick(label);
    assert!(matches!(
        answer_and_tap(bus, thread_id, &card, picked).await,
        Resolution::Granted(_)
    ));
}

pub(super) fn create(slug: &str) -> Ask {
    workspace_ask(Action::CreateWorkspace, slug, "A film take needs it.").unwrap()
}

pub(super) async fn live_create(
    bus: &EventBus,
    thread_id: Uuid,
    slug: &str,
) -> Option<PermissionRow> {
    live_permission(
        bus.pool(),
        &thread_user(thread_id),
        Action::CreateWorkspace,
        Some(slug),
    )
    .await
    .unwrap()
}

#[cfg(feature = "test-support")]
pub async fn seed_snapshot_grant(bus: &EventBus) -> Uuid {
    let thread = agent_thread(bus).await;
    grant(bus, thread, create("tour-film-a"), ALWAYS_ALLOW_LABEL).await;
    thread
}

#[cfg(feature = "test-support")]
pub async fn assert_snapshot_grant_expired(bus: &EventBus, thread: Uuid) {
    assert!(live_create(bus, thread, "tour-film-a").await.is_none());
}
