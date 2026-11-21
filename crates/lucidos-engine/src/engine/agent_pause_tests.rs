use super::*;
use crate::test_support::{setup_test_db, teardown_test_db};

use MessageAudience::{Agent, UserOnly};
use PauseChange::{Pause, Resume};

/// Every combination of (users mentioned, agent mentioned, paused, question
/// open).
#[test]
fn every_mention_and_pause_combination_has_one_route() {
    let table = [
        // users, agent, paused => pause change, audience
        ((false, false, false), (None, Agent)),
        ((false, false, true), (None, UserOnly)),
        ((true, false, false), (Some(Pause), UserOnly)),
        ((true, false, true), (None, UserOnly)),
        ((false, true, false), (None, Agent)),
        ((false, true, true), (Some(Resume), Agent)),
        ((true, true, false), (None, Agent)),
        ((true, true, true), (Some(Resume), Agent)),
    ];
    for ((users, agent, paused), (pause_change, audience)) in table {
        // An open card changes only the untagged message on a paused thread.
        for question_open in [false, true] {
            let audience = match (users, agent, paused, question_open) {
                (false, false, true, true) => Agent,
                _ => audience,
            };
            assert_eq!(
                user_only_message_route(users, agent, paused, question_open),
                UserOnlyMessageRoute {
                    pause_change,
                    audience
                },
                "users={users} agent={agent} paused={paused} question_open={question_open}"
            );
        }
    }
}

fn device(id: &str) -> Option<MessageOrigin> {
    Some(MessageOrigin::Device {
        device_id: id.into(),
    })
}

/// The pause lives only in events, so a fresh connection after a restart
/// reads the same state. A chip repeat records nothing.
#[tokio::test]
async fn the_pause_folds_from_events_and_survives_a_restart() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    assert!(!agent_paused(&pool, thread).await.unwrap());

    record_pause_change(
        &bus,
        thread,
        Pause,
        AgentPauseCause::Mention,
        device("sam-phone"),
    )
    .await
    .unwrap();
    assert!(agent_paused(&pool, thread).await.unwrap());
    assert!(
        set_agent_paused(&pool, &bus, thread, true, device("kim-phone"))
            .await
            .unwrap()
    );
    let flips: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE thread_id = $1 \
         AND event_type IN ('AgentPaused', 'AgentResumed')",
    )
    .bind(thread)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(flips, 1, "a chip repeat records nothing");
    drop(bus);

    let options = (*pool.connect_options()).clone();
    pool.close().await;
    let reopened = PgPool::connect_with(options).await.unwrap();
    assert!(agent_paused(&reopened, thread).await.unwrap());

    let (bus, _rx) = EventBus::new(reopened.clone());
    assert!(
        !set_agent_paused(&reopened, &bus, thread, false, device("kim-phone"))
            .await
            .unwrap()
    );
    assert!(!agent_paused(&reopened, thread).await.unwrap());

    reopened.close().await;
    teardown_test_db(&db_name).await;
}
