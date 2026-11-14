//! Each tool-permission card, read as a permission request, offers exactly the
//! buttons the card draws and maps each to its typed choice (I9).

use super::*;
use crate::engine::event_bus::EventBus;
use crate::engine::thread_events::ThreadEvent;
use crate::test_support::{seed_thread_event, setup_test_db, teardown_test_db};

fn labels(offer: &GrantOffer) -> Vec<String> {
    offered_picks(offer)
        .into_iter()
        .map(|(_, label, _)| label)
        .collect()
}

/// A bash command with a head draws all five buttons, as `PermissionCard.tsx`
/// does. Each Always allow names the line the card's resolver stores.
#[test]
fn a_bash_card_offers_every_button_and_names_its_lines() {
    let offer = command_offer("run_bash", "git push origin main", None);
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
    let headed = command_offer("run_bash", "cd /work && git push", Some("git"));
    assert_eq!(
        headed.narrow.as_deref(),
        Some("Bash(git:*)"),
        "the card's own head"
    );
}

/// Python has one line, so it draws no narrow button. MCP draws all five, and
/// a coding-agent card draws only what its offer stamped.
#[test]
fn each_lane_offers_the_buttons_its_card_draws() {
    assert_eq!(
        labels(&command_offer("run_python", "print(1)", None)),
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

/// Each pick maps to the choice its lifetime names. Once and this thread
/// stay with the thread, and an Always allow stands for the workspace.
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
        offer: command_offer("run_bash", "git push", None),
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

/// The reader finds every open card on the thread, newest first, and drops a
/// resolved one.
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
        request_id: request_id.into(),
        tool_use_id: format!("toolu-{request_id}"),
        tool_name: "run_bash".into(),
        command: "gh release delete v1".into(),
        summary: "Deletes a published release.".into(),
        grant_head: None,
        grant_offer: None,
    };
    seed_thread_event(&bus, thread_id, command("req-old")).await;
    seed_thread_event(
        &bus,
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
    seed_thread_event(&bus, thread_id, command("req-done")).await;
    seed_thread_event(
        &bus,
        thread_id,
        ThreadEvent::CommandPermissionResolved {
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
