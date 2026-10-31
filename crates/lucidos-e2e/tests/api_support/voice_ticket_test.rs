//! The voice socket's *call ticket*, against a booted engine.
//!
//! A browser sends no `Referer` on a socket handshake, so an app page on the
//! engine's origin could dial `/api/v1/voice` as the owner. The upgrade now
//! redeems a ticket from `POST /api/v1/voice/ticket`, a plain `fetch` that
//! runs the strict origin rule (ADR 0598).
//!
//! No test here lets an upgrade succeed: a live call would reach a real voice
//! provider. A ticket is spent on another thread instead, then replayed on home.

use crate::support::{base_url, browser_handshake, db_url, user_client};
use lucidos_engine::core::prefs;
use reqwest::StatusCode;
use serde_json::{json, Value};
use uuid::Uuid;

async fn home_thread() -> Uuid {
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database");
    let home = sqlx::query_scalar("SELECT thread_id FROM thread_summaries WHERE is_home")
        .fetch_one(&pool)
        .await
        .expect("the workspace has a home thread");
    pool.close().await;
    home
}

/// Dial `thread_id` as the registered device, with `ticket` if given. The
/// `ticket` query name pins the wire contract the client dials with.
async fn dial(client: &reqwest::Client, thread_id: Uuid, ticket: Option<&str>) -> StatusCode {
    let mut path = format!("/api/v1/voice?thread_id={thread_id}");
    if let Some(ticket) = ticket {
        path.push_str(&format!("&ticket={ticket}"));
    }
    browser_handshake(client, &path, &base_url())
        .send()
        .await
        .expect("upgrade request failed")
        .status()
}

/// A browser-shaped mint, naming the page it came from if `referer` is given.
async fn mint(
    client: &reqwest::Client,
    thread_id: Uuid,
    referer: Option<&str>,
) -> reqwest::Response {
    let mut request = client
        .post(format!("{}/api/v1/voice/ticket", base_url()))
        .header("sec-fetch-site", "same-origin")
        .json(&json!({ "thread_id": thread_id }));
    if let Some(referer) = referer {
        request = request.header("referer", referer);
    }
    request.send().await.expect("mint request failed")
}

/// Turns voice on, or restores the default on `None`. The handler answers 200
/// on failure too, so the body's `success` is the result. A delete of an unset
/// key reports "not found", which is already the default.
async fn set_voice_enabled(client: &reqwest::Client, on: Option<bool>) {
    let url = format!(
        "{}/api/v1/preferences?key={}",
        base_url(),
        prefs::VOICE_ENABLED.key()
    );
    let body: Value = match on {
        Some(on) => client
            .put(url)
            .json(&json!({ "value": on.to_string() }))
            .send(),
        None => client.delete(url).send(),
    }
    .await
    .expect("voice switch write failed")
    .json()
    .await
    .expect("voice switch reply is not JSON");
    let unset = on.is_none()
        && body["error"]
            .as_str()
            .is_some_and(|e| e.ends_with("not found"));
    assert!(body["success"] == true || unset, "{body}");
}

#[tokio::test]
async fn an_upgrade_without_a_ticket_is_refused() {
    let client = user_client().await;
    assert_eq!(
        dial(&client, home_thread().await, None).await,
        StatusCode::FORBIDDEN,
        "the owner's device header alone must not open a call"
    );
}

#[tokio::test]
async fn an_app_document_cannot_mint_a_ticket() {
    let client = user_client().await;
    let home = home_thread().await;
    let app_referer = format!("{}/app/habit-tracker/", base_url());
    assert_eq!(
        mint(&client, home, Some(&app_referer)).await.status(),
        StatusCode::FORBIDDEN,
        "a mint from an app document must be refused"
    );
    assert_eq!(
        mint(&client, home, None).await.status(),
        StatusCode::FORBIDDEN,
        "a browser-shaped mint that names no page must be refused"
    );
}

/// A ticket opens only the thread it names, and spends on the first try.
///
/// Voice is a global switch. The steps run in their own task, so a panic there
/// still reaches the restore before anything asserts.
#[tokio::test]
async fn a_ticket_spent_on_another_thread_cannot_then_open_home() {
    let client = user_client().await;
    let home = home_thread().await;

    set_voice_enabled(&client, Some(true)).await;
    let steps = tokio::spawn({
        let client = client.clone();
        async move {
            let minted = mint(&client, home, Some(&format!("{}/", base_url()))).await;
            let mint_status = minted.status();
            let body: Value = minted.json().await.unwrap_or_default();
            let ticket = body["ticket"].as_str().unwrap_or_default().to_string();
            let elsewhere = dial(&client, Uuid::new_v4(), Some(&ticket)).await;
            let replay = dial(&client, home, Some(&ticket)).await;
            (mint_status, body, ticket, elsewhere, replay)
        }
    })
    .await;
    set_voice_enabled(&client, None).await;
    let (mint_status, body, ticket, elsewhere, replay) = steps.expect("the call steps panicked");

    assert_eq!(mint_status, StatusCode::OK, "the shell's mint: {body}");
    assert!(!ticket.is_empty(), "the mint answers a ticket: {body}");
    // Pins the wire contract: the client is told a ticket lives 30 seconds.
    assert_eq!(body["expires_in_secs"], 30, "{body}");
    assert_eq!(
        elsewhere,
        StatusCode::FORBIDDEN,
        "a ticket opens its own thread only"
    );
    assert_eq!(
        replay,
        StatusCode::FORBIDDEN,
        "the wrong-thread try spent the ticket"
    );
}
