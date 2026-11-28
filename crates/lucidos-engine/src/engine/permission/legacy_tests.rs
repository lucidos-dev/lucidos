use super::*;
use crate::engine::event_bus::EventBus;
use crate::engine::permission::{Action, Holder, Lifetime, PermissionChoice};
use crate::engine::thread_events::ThreadEvent;
use crate::test_support::{seed_pre_gate_card, seed_thread_event, setup_test_db, teardown_test_db};

fn labels(offer: &GrantOffer) -> Vec<String> {
    offered_picks(offer)
        .into_iter()
        .map(|(_, label, _)| label)
        .collect()
}

#[test]
fn unstamped_command_cards_offer_no_grant() {
    for tool in ["run_bash", "run_bash_background", "run_python"] {
        let card = LegacyCard::from_payload(
            LegacyLane::Command,
            Uuid::new_v4(),
            1,
            &serde_json::json!({
                "request_id": "old-card", "tool_name": tool,
                "command": "git push", "grant_head": "git"
            }),
        )
        .unwrap();
        assert_eq!(labels(&card.offer), ["Allow once", "Deny"], "{tool}");
    }
}

fn stamped_card(tool: &str, command: &str, offer: GrantOffer) -> LegacyCard {
    LegacyCard::from_payload(
        LegacyLane::Command,
        Uuid::new_v4(),
        1,
        &serde_json::json!({
            "request_id": "stamped-card", "tool_name": tool,
            "command": command, "grant_offer": offer,
        }),
    )
    .unwrap()
}

fn bash_offer() -> GrantOffer {
    GrantOffer {
        deny: None,
        session: Some("Bash(git:*)".into()),
        narrow: Some("Bash(git:*)".into()),
        broad: Some("Bash".into()),
        withheld: None,
    }
}

#[test]
fn a_stamped_secret_read_offers_no_grant() {
    use crate::engine::thread_events::OfferWithheld;
    let card = stamped_card(
        "run_bash",
        "cat ~/.lucidos/gateway/gateway.log",
        GrantOffer::withheld(OfferWithheld::SecretRead),
    );
    assert_eq!(labels(&card.offer), ["Allow once", "Deny"]);
    for scope in [LegacyPick::Session, LegacyPick::Narrow, LegacyPick::Broad] {
        assert_eq!(card.pick_of(&scope.option_id()), None);
        assert!(!card.request().choices.contains_key(&scope.option_id()));
    }
}

#[test]
fn an_unproven_floor_match_offers_no_grant_and_says_why() {
    use crate::engine::thread_events::OfferWithheld;
    let card = stamped_card(
        "run_bash",
        "make",
        GrantOffer::withheld(OfferWithheld::UnprovenWorkspaceDestruction),
    );
    assert_eq!(labels(&card.offer), ["Allow once", "Deny"]);
    assert!(
        card.question.contains("deletes a Lucidos workspace"),
        "{}",
        card.question
    );
}

#[test]
fn a_bash_card_offers_every_button_and_names_its_lines() {
    let offer = stamped_card("run_bash", "git push origin main", bash_offer()).offer;
    assert_eq!(
        labels(&offer),
        [
            "Allow once",
            "Allow for this thread",
            "Always allow `Bash(git:*)`",
            "Always allow `Bash`",
            "Deny",
        ]
    );
    let headed = stamped_card("run_bash", "cd /work && git push", bash_offer()).offer;
    assert_eq!(
        headed.narrow.as_deref(),
        Some("Bash(git:*)"),
        "the card's own head"
    );
}

#[test]
fn each_lane_offers_the_buttons_its_card_draws() {
    assert_eq!(
        labels(
            &stamped_card(
                "run_python",
                "print(1)",
                GrantOffer {
                    session: Some("Python".into()),
                    broad: Some("Python".into()),
                    ..GrantOffer::default()
                }
            )
            .offer
        ),
        [
            "Allow once",
            "Allow for this thread",
            "Always allow `Python`",
            "Deny"
        ]
    );
    assert_eq!(
        labels(&mcp_offer("example-server", "post_message")),
        [
            "Allow once",
            "Allow for this thread",
            "Always allow `Mcp(example-server:post_message)`",
            "Always allow `Mcp(example-server:*)`",
            "Deny",
        ]
    );
    assert_eq!(
        labels(&GrantOffer::default()),
        ["Allow once", "Deny"],
        "an old coding-agent card stamped no offer"
    );
}

#[test]
fn each_pick_maps_to_its_typed_choice() {
    let thread_id = Uuid::new_v4();
    let card = LegacyCard {
        lane: LegacyLane::Command,
        thread_id,
        request_id: "r1".into(),
        sequence: 1,
        question: "Pushes.\ngit push".into(),
        tool_name: "run_bash".into(),
        offer: bash_offer(),
    };
    let request = card.request();
    assert_eq!(request.action, Action::RunCommand);
    assert_eq!(request.breadth.as_deref(), Some("run_bash"));
    let expected = [
        (LegacyPick::Once, Lifetime::Once),
        (LegacyPick::Session, Lifetime::ThisThread),
        (LegacyPick::Narrow, Lifetime::Standing),
        (LegacyPick::Broad, Lifetime::Standing),
    ];
    for (pick, lifetime) in expected {
        let (_, option) = card
            .options()
            .into_iter()
            .find(|(p, _)| *p == pick)
            .unwrap();
        let holder = if lifetime == Lifetime::Standing {
            Holder::Workspace
        } else {
            Holder::Thread { thread_id }
        };
        assert_eq!(
            request.choices.get(&option.id),
            Some(&PermissionChoice::grant(lifetime, holder)),
            "{pick:?}"
        );
        assert_eq!(card.pick_of(&option.id), Some(pick));
    }
    assert_eq!(
        request.choices.get(&LegacyPick::Deny.option_id()),
        Some(&PermissionChoice::Refuse)
    );
    assert_eq!(LegacyLane::CodingAgent.action(), Action::UseTool);
    assert_eq!(LegacyLane::Mcp.action(), Action::UseMcpTool);
    for pick in LegacyPick::ALL {
        assert_eq!(pick.allowed(), pick != LegacyPick::Deny);
    }
}

#[tokio::test]
async fn the_reader_finds_each_open_card_newest_first() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    sqlx::query("INSERT INTO thread_summaries (thread_id, source) VALUES ($1, 'chat')")
        .bind(thread_id)
        .execute(&pool)
        .await
        .unwrap();
    seed_thread_event(
        &bus,
        thread_id,
        ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "post the release notes".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: crate::engine::thread_events::ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
    )
    .await;
    let command = |request_id: &str| ThreadEvent::CommandPermissionRequested {
        decision_trace: None,
        request_id: request_id.into(),
        tool_use_id: format!("toolu-{request_id}"),
        tool_name: "run_bash".into(),
        command: "gh release delete v1".into(),
        summary: "Deletes a published release.".into(),
        grant_head: None,
        grant_offer: Some(GrantOffer {
            narrow: Some("Bash(gh:*)".into()),
            ..GrantOffer::default()
        }),
    };
    seed_thread_event(&bus, thread_id, command("req-old")).await;
    seed_pre_gate_card(
        &pool,
        thread_id,
        ThreadEvent::McpPermissionRequested {
            request_id: "req-mcp".into(),
            tool_use_id: "toolu-mcp".into(),
            server_id: "example-server".into(),
            server_name: "Example Server".into(),
            tool_name: "post_message".into(),
            arguments_summary: "{}".into(),
        },
    )
    .await;
    seed_pre_gate_card(&pool, thread_id, command("req-done")).await;
    seed_thread_event(
        &bus,
        thread_id,
        ThreadEvent::CommandPermissionResolved {
            standing_deny: None,
            request_id: "req-done".into(),
            allowed: true,
            reason: None,
            persist_scope: None,
        },
    )
    .await;

    let cards = open_cards(&pool, thread_id).await.unwrap();
    let ids: Vec<&str> = cards.iter().map(|c| c.request_id.as_str()).collect();
    assert_eq!(ids, ["req-mcp", "req-old"]);
    assert_eq!(cards[0].lane, LegacyLane::Mcp);
    assert_eq!(
        cards[0].question,
        "Example Server wants to use its post_message tool.\n{}"
    );
    assert_eq!(cards[1].offer.narrow.as_deref(), Some("Bash(gh:*)"));
    teardown_test_db(&db).await;
}
