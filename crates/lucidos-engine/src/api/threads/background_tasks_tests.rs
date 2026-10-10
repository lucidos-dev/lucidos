//! Who may reach a thread's background tasks, and what the start response
//! tells the agent to do next.

use super::*;
use crate::api::actor::{
    init_agent_origin_secret, mint_agent_origin_token, HEADER_AGENT_ORIGIN_TOKEN,
};

/// Headers as a Lucidos-spawned subprocess sends them. The secret is installed
/// first, or minting returns `None` and every caller reads as untokened.
fn agent_headers(thread_id: Option<Uuid>) -> HeaderMap {
    init_agent_origin_secret("harden-test-secret".to_string());
    let mut h = HeaderMap::new();
    let token = mint_agent_origin_token(thread_id, 0, None)
        .expect("the secret is installed above, so minting cannot fail");
    h.insert(HEADER_AGENT_ORIGIN_TOKEN, token.parse().unwrap());
    h
}

/// The route runs a shell command, so it serves exactly one caller: the
/// thread's own agent. Another thread's agent, a thread-less subprocess and a
/// caller with no token are all refused.
#[test]
fn only_the_threads_own_agent_may_reach_its_background_tasks() {
    let mine = Uuid::new_v4();
    let theirs = Uuid::new_v4();
    assert!(require_own_thread_agent(&agent_headers(Some(mine)), mine).is_ok());

    for (label, headers) in [
        ("another thread's agent", agent_headers(Some(theirs))),
        ("a subprocess with no thread", agent_headers(None)),
        ("a caller with no token", HeaderMap::new()),
    ] {
        let refused = require_own_thread_agent(&headers, mine)
            .expect_err(&format!("{label} must be refused"));
        assert_eq!(refused.0, StatusCode::FORBIDDEN, "{label}");
    }
}

/// Each outcome has a different right next move, and the message is the only
/// thing the agent reads. Watched: end the turn. Unwatched: do not.
#[test]
fn the_start_response_tells_the_agent_what_to_do_next() {
    let watched = start_response(BackgroundTaskStart::Watched {
        task_id: "t1".into(),
        timeout_secs: 3600,
    });
    assert_eq!(watched["status"], "watched");
    let msg = watched["message"].as_str().unwrap();
    assert!(msg.contains("end your turn now"), "{msg}");
    assert!(msg.contains("lucidos background-task output t1"), "{msg}");

    let unwatched = start_response(BackgroundTaskStart::Unwatched {
        task_id: "t2".into(),
        timeout_secs: 3600,
    });
    assert_eq!(unwatched["status"], "unwatched");
    let msg = unwatched["message"].as_str().unwrap();
    assert!(msg.contains("do not end your turn"), "{msg}");
    assert!(msg.contains("lucidos background-task stop t2"), "{msg}");

    let finished = start_response(BackgroundTaskStart::Finished {
        task_id: "t3".into(),
        output: r#"{"exit_code":0,"stdout":"ok"}"#.into(),
    });
    assert_eq!(finished["status"], "finished");
    assert_eq!(
        finished["output"]["exit_code"], 0,
        "output is embedded as JSON, not as a string"
    );
}
