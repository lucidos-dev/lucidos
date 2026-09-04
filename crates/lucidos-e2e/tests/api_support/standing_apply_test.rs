//! The *standing apply* over HTTP: arm, read back, disarm.
//!
//! ADR 0168 clause 5. The engine-side verdict is unit-tested in
//! `engine::standing_apply`; this covers the surface the buttons press.

use crate::support::{base_url, db_url, http_client, seed_cc_thread_summary, user_client};
use serde_json::json;
use uuid::Uuid;

async fn seed_change(pool: &sqlx::PgPool, change_id: Uuid, thread_id: Uuid, branch: &str) {
    sqlx::query(
        "INSERT INTO changes \
           (id, request_id, thread_id, branch_name, repo_root, description, \
            file_count, files, requires_restart, status) \
         VALUES ($1, $2, $3, $4, '/repo', 'e2e standing apply', 2, \
                 ARRAY['a.rs'], false, 'pending')",
    )
    .bind(change_id)
    .bind(Uuid::new_v4())
    .bind(thread_id)
    .bind(branch)
    .execute(pool)
    .await
    .expect("seed change");
}

async fn armed_threads(client: &reqwest::Client) -> Vec<String> {
    let body: serde_json::Value = client
        .get(format!("{}/api/v1/changes", base_url()))
        .send()
        .await
        .expect("changes request failed")
        .json()
        .await
        .expect("changes body");
    body["standing_apply_thread_ids"]
        .as_array()
        .expect("standing_apply_thread_ids must be an array")
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

/// The round trip: arm a working thread, see it in the changes payload, take it
/// back. That payload is what every surface reads to draw the armed face, so a
/// silent omission there is an armed thread nobody can disarm.
#[tokio::test]
async fn arming_a_standing_apply_shows_up_and_can_be_taken_back() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");

    let thread_id = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    seed_cc_thread_summary(&pool, thread_id, "running").await;
    seed_change(
        &pool,
        change_id,
        thread_id,
        &format!(
            "e2e-test/standing-{}",
            &change_id.as_simple().to_string()[..8]
        ),
    )
    .await;

    let resp = client
        .post(format!("{}/api/v1/standing-applies", base_url()))
        .json(&json!({ "thread_id": thread_id, "change_id": change_id }))
        .send()
        .await
        .expect("arm request failed");
    assert!(
        resp.status().is_success(),
        "arm returned {}: {}",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );

    assert!(
        armed_threads(&client)
            .await
            .contains(&thread_id.to_string()),
        "the armed thread must appear in the changes payload"
    );

    let resp = client
        .delete(format!(
            "{}/api/v1/standing-applies/{}",
            base_url(),
            thread_id
        ))
        .send()
        .await
        .expect("disarm request failed");
    assert!(
        resp.status().is_success(),
        "disarm returned {}",
        resp.status()
    );

    assert!(
        !armed_threads(&client)
            .await
            .contains(&thread_id.to_string()),
        "a disarmed thread must leave the payload"
    );
}

/// An arm names a change, and that change must be the named thread's own.
/// Binding one thread's arm to another's change would apply, on this thread's
/// settle, work the owner never looked at.
#[tokio::test]
async fn an_arm_refuses_a_change_belonging_to_another_thread() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");

    let mine = Uuid::new_v4();
    let theirs = Uuid::new_v4();
    let their_change = Uuid::new_v4();
    seed_cc_thread_summary(&pool, mine, "running").await;
    seed_cc_thread_summary(&pool, theirs, "running").await;
    seed_change(
        &pool,
        their_change,
        theirs,
        &format!(
            "e2e-test/standing-{}",
            &their_change.as_simple().to_string()[..8]
        ),
    )
    .await;

    let resp = client
        .post(format!("{}/api/v1/standing-applies", base_url()))
        .json(&json!({ "thread_id": mine, "change_id": their_change }))
        .send()
        .await
        .expect("arm request failed");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::BAD_REQUEST,
        "arming across threads must be refused"
    );
    assert!(
        !armed_threads(&client).await.contains(&mine.to_string()),
        "a refused arm must leave nothing behind"
    );
}

/// A standing apply is an Apply, one settle later, so it is offered exactly
/// where Apply is. Lucidos never merges into an external repo. Such a thread
/// proposes nothing either, so an arm on one waits for a change that never
/// comes.
#[tokio::test]
async fn an_arm_refuses_a_repo_lucidos_never_applies_into() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");

    let thread_id = Uuid::new_v4();
    seed_cc_thread_summary(&pool, thread_id, "running").await;
    sqlx::query(
        "UPDATE thread_summaries \
            SET coding_agent_kind = 'external', coding_agent_is_external_repo = TRUE \
          WHERE thread_id = $1",
    )
    .bind(thread_id)
    .execute(&pool)
    .await
    .expect("mark the thread as external-repo");

    let resp = client
        .post(format!("{}/api/v1/standing-applies", base_url()))
        .json(&json!({ "thread_id": thread_id }))
        .send()
        .await
        .expect("arm request failed");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::BAD_REQUEST,
        "arming an external-repo thread must be refused"
    );
    assert!(
        !armed_threads(&client)
            .await
            .contains(&thread_id.to_string()),
        "a refused arm must leave nothing behind"
    );
}

/// The workspace-scope off, which the Changes panel's toggle presses. It takes
/// back every arm here, whether a sweep set it or the owner armed one change.
///
/// Both threads must leave `standing_apply_thread_ids`, because that payload is
/// the only armed signal any surface reads.
#[tokio::test]
async fn the_workspace_off_takes_back_every_arm() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to the e2e workspace database");

    let bound = Uuid::new_v4();
    let unbound = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    seed_cc_thread_summary(&pool, bound, "running").await;
    seed_cc_thread_summary(&pool, unbound, "running").await;
    seed_change(
        &pool,
        change_id,
        bound,
        &format!(
            "e2e-test/standing-all-{}",
            &change_id.as_simple().to_string()[..8]
        ),
    )
    .await;

    for body in [
        json!({ "thread_id": bound, "change_id": change_id }),
        json!({ "thread_id": unbound }),
    ] {
        let resp = client
            .post(format!("{}/api/v1/standing-applies", base_url()))
            .json(&body)
            .send()
            .await
            .expect("arm request failed");
        assert!(resp.status().is_success(), "arm returned {}", resp.status());
    }

    let armed = armed_threads(&client).await;
    assert!(armed.contains(&bound.to_string()) && armed.contains(&unbound.to_string()));

    let resp = client
        .delete(format!("{}/api/v1/standing-applies", base_url()))
        .send()
        .await
        .expect("workspace disarm request failed");
    assert!(
        resp.status().is_success(),
        "workspace disarm returned {}",
        resp.status()
    );

    let left = armed_threads(&client).await;
    assert!(
        !left.contains(&bound.to_string()) && !left.contains(&unbound.to_string()),
        "the workspace off must leave nothing armed, got {left:?}"
    );
}

/// A caller presenting no credential cannot cancel the owner's instruction.
/// `api::mutating_gate` answers before the handler (ADR 0169).
#[tokio::test]
async fn an_unidentified_disarm_is_refused() {
    let client = http_client();
    for url in [
        format!("{}/api/v1/standing-applies", base_url()),
        format!("{}/api/v1/standing-applies/{}", base_url(), Uuid::new_v4()),
    ] {
        let resp = client
            .delete(&url)
            .send()
            .await
            .expect("disarm request failed");
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "{url} must refuse a caller that named nobody"
        );
    }
}

/// Disarming a thread that carries nothing is a 404, not a silent success. The
/// caller asked to take back an instruction that was not there.
#[tokio::test]
async fn disarming_an_unarmed_thread_is_a_not_found() {
    let client = user_client().await;
    let resp = client
        .delete(format!(
            "{}/api/v1/standing-applies/{}",
            base_url(),
            Uuid::new_v4()
        ))
        .send()
        .await
        .expect("disarm request failed");
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);
}
