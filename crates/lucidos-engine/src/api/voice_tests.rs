//! The call ticket's mint and the upgrade that redeems it.
//!
//! Every refusal a person can act on is driven through the mint, since that is
//! where our client reads it. The upgrade's tests are about the ticket itself
//! and the re-check under it.

use super::*;
use crate::api::actor::{
    init_agent_origin_secret, mint_agent_origin_token, HEADER_AGENT_ORIGIN_TOKEN, HEADER_DEVICE_ID,
};
use crate::api::voice_ticket::{mint, CallTickets, NOT_FROM_THE_SHELL, QUERY_CALL_TICKET};
use crate::engine::thread_events::MessageOrigin;
use crate::test_support::{setup_test_db, teardown_test_db};
use crate::voice::registry::LiveVoiceSessions;

const OWNERS_PHONE: &str = "owners-phone";
const OWNERS_LAPTOP: &str = "owners-laptop";

fn headers_with(pairs: &[(&'static str, &str)]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in pairs {
        headers.insert(*name, value.parse().expect("a valid header value"));
    }
    headers
}

fn device_headers(device_id: &str) -> HeaderMap {
    headers_with(&[(HEADER_DEVICE_ID, device_id)])
}

/// The upgrade's own URI, carrying `ticket` when there is one.
fn call_uri(thread_id: Uuid, ticket: Option<&str>) -> Uri {
    let ticket = ticket.map_or(String::new(), |t| format!("&{QUERY_CALL_TICKET}={t}"));
    format!("/voice?thread_id={thread_id}{ticket}")
        .parse()
        .expect("a valid URI")
}

/// The stores a call touches, fresh per test.
struct Line {
    sessions: LiveVoiceSessions,
    tickets: CallTickets,
}

impl Line {
    fn new() -> Self {
        Self {
            sessions: LiveVoiceSessions::new(),
            tickets: CallTickets::new(),
        }
    }

    /// The owner asks their registered phone for a ticket.
    async fn owner_mints(&self, pool: &sqlx::PgPool, thread_id: Uuid) -> Result<String, ApiError> {
        crate::test_support::seed_device(pool, OWNERS_PHONE, Some("Mozilla/5.0"), None).await;
        mint(
            pool,
            &self.sessions,
            &self.tickets,
            &device_headers(OWNERS_PHONE),
            thread_id,
        )
        .await
    }

    /// Dial `thread_id` with `ticket`, as the gateway forwards it.
    async fn dial(
        &self,
        pool: &sqlx::PgPool,
        headers: &HeaderMap,
        thread_id: Uuid,
        ticket: Option<&str>,
    ) -> Result<Admitted, ApiError> {
        admit(
            pool,
            &self.sessions,
            &self.tickets,
            headers,
            &call_uri(thread_id, ticket),
            thread_id,
        )
        .await
    }

    /// The whole call: mint from the phone, then dial from it.
    async fn owner_calls(
        &self,
        pool: &sqlx::PgPool,
        thread_id: Uuid,
    ) -> Result<Admitted, ApiError> {
        let ticket = self.owner_mints(pool, thread_id).await?;
        self.dial(
            pool,
            &device_headers(OWNERS_PHONE),
            thread_id,
            Some(&ticket),
        )
        .await
    }
}

async fn a_chat_thread(pool: &sqlx::PgPool) -> Uuid {
    let thread_id = Uuid::new_v4();
    sqlx::query("INSERT INTO thread_summaries (thread_id, source) VALUES ($1, 'chat')")
        .bind(thread_id)
        .execute(pool)
        .await
        .expect("create the thread");
    thread_id
}

/// The workspace's home thread, the one thread a call runs on.
async fn the_home_thread(pool: &sqlx::PgPool) -> Uuid {
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    crate::engine::home_thread::ensure_home_thread(&bus, pool)
        .await
        .expect("create the home thread")
}

/// A thread a coding agent holds.
///
/// `source` reads `claude_code` for a started one and for a DRAFT alike.
/// The compose write mirrors the picked mode into it, so a draft says who
/// holds it before it carries a single message.
async fn a_coding_agent_thread(pool: &sqlx::PgPool, state: &str) -> Uuid {
    let thread_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO thread_summaries (thread_id, source, state) VALUES ($1, 'claude_code', $2)",
    )
    .bind(thread_id)
    .bind(state)
    .execute(pool)
    .await
    .expect("create the thread");
    thread_id
}

/// Opt this workspace into voice, or back out of it.
async fn voice_set(pool: &sqlx::PgPool, on: bool) {
    let key = crate::core::prefs::VOICE_ENABLED.key();
    crate::core::PreferenceStore::set_row_for_test(pool, key, if on { "true" } else { "false" })
        .await
        .expect("set the voice switch");
}

/// Voice ships off. A workspace that never opted in has no call to place,
/// and the refusal lands before the thread is even looked up. Every
/// workspace has Home, so the voice switch is the only one it names.
#[tokio::test]
async fn a_ticket_is_refused_while_voice_is_off() {
    let (pool, db_name) = setup_test_db().await;
    let line = Line::new();
    let thread_id = the_home_thread(&pool).await;

    let error = line
        .owner_mints(&pool, thread_id)
        .await
        .expect_err("should refuse");
    assert_eq!(error.status, StatusCode::FORBIDDEN);
    assert!(error.message.contains("under Voice"), "{}", error.message);

    // And it is the switch that decides, not the thread.
    voice_set(&pool, true).await;
    line.owner_calls(&pool, thread_id)
        .await
        .expect("a call once voice is on");

    teardown_test_db(&db_name).await;
}

/// A call reaches the Lucidos Agent and nothing else (ADR 0165).
///
/// Both states, because the draft is the one the user actually hits: they
/// pick Claude Code in the compose view and press the control before the
/// thread has a single message in it.
///
/// The message names the way back rather than saying only "no". The
/// destination picker sits in the same row as the control they pressed.
#[tokio::test]
async fn a_ticket_for_a_coding_agent_thread_is_refused() {
    let (pool, db_name) = setup_test_db().await;
    let line = Line::new();
    voice_set(&pool, true).await;

    for state in ["composing", "active"] {
        let thread_id = a_coding_agent_thread(&pool, state).await;
        let error = line
            .owner_mints(&pool, thread_id)
            .await
            .err()
            .unwrap_or_else(|| panic!("should refuse a {} coding-agent thread", state));
        assert_eq!(error.status, StatusCode::FORBIDDEN);
        assert!(
            error.message.contains("Switch the destination"),
            "the refusal must name the way back: {}",
            error.message
        );
    }

    teardown_test_db(&db_name).await;
}

/// Voice sessions live in the home thread alone (ADR 0362, invariant I12).
/// Any other Lucidos Agent thread is refused, and the message names where
/// to go instead.
#[tokio::test]
async fn a_ticket_for_any_thread_but_home_is_refused() {
    let (pool, db_name) = setup_test_db().await;
    let line = Line::new();
    voice_set(&pool, true).await;
    let home = the_home_thread(&pool).await;
    let other = a_chat_thread(&pool).await;

    let error = line
        .owner_mints(&pool, other)
        .await
        .expect_err("should refuse a chat thread that is not home");
    assert_eq!(error.status, StatusCode::FORBIDDEN);
    assert!(error.message.contains("home thread"), "{}", error.message);

    line.owner_calls(&pool, home)
        .await
        .expect("a call on the home thread");

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_ticket_for_a_thread_that_does_not_exist_is_refused() {
    let (pool, db_name) = setup_test_db().await;
    let line = Line::new();
    voice_set(&pool, true).await;

    let error = line
        .owner_mints(&pool, Uuid::new_v4())
        .await
        .expect_err("should refuse");
    assert_eq!(error.status, StatusCode::NOT_FOUND);

    teardown_test_db(&db_name).await;
}

/// One live session per thread. It is what keeps every start paired with
/// exactly one end, so the second caller is refused rather than queued. The
/// mint refuses it, and so does a dial on a ticket minted before the first
/// call began.
#[tokio::test]
async fn a_second_call_on_a_busy_thread_is_refused() {
    let (pool, db_name) = setup_test_db().await;
    let line = Line::new();
    voice_set(&pool, true).await;
    let thread_id = the_home_thread(&pool).await;

    let early = line.owner_mints(&pool, thread_id).await.expect("a ticket");
    let Admitted {
        session_id: first_id,
        slot: held,
        ..
    } = line
        .owner_calls(&pool, thread_id)
        .await
        .expect("first call");

    let error = line
        .owner_mints(&pool, thread_id)
        .await
        .expect_err("the mint should refuse a busy thread");
    assert_eq!(error.status, StatusCode::CONFLICT);
    let error = line
        .dial(
            &pool,
            &device_headers(OWNERS_PHONE),
            thread_id,
            Some(&early),
        )
        .await
        .err()
        .expect("the dial should refuse a busy thread");
    assert_eq!(error.status, StatusCode::CONFLICT);
    assert_eq!(
        line.sessions.count(),
        1,
        "the refusal must not evict the first"
    );

    // Ringing off frees the thread, and the next call gets a fresh id.
    drop(held);
    let second_id = line
        .owner_calls(&pool, thread_id)
        .await
        .expect("next call")
        .session_id;
    assert_ne!(first_id, second_id);

    teardown_test_db(&db_name).await;
}

/// A ticket mint names its caller like any write. No credential, or a device
/// id naming no registered row, is refused.
#[tokio::test]
async fn a_caller_naming_nobody_gets_no_ticket() {
    let (pool, db_name) = setup_test_db().await;
    let line = Line::new();
    voice_set(&pool, true).await;
    let home = the_home_thread(&pool).await;

    for headers in [HeaderMap::new(), device_headers("nobody-registered")] {
        let error = mint(&pool, &line.sessions, &line.tickets, &headers, home)
            .await
            .expect_err("should refuse an unidentified caller");
        assert_eq!(error.status, StatusCode::UNAUTHORIZED);
    }

    teardown_test_db(&db_name).await;
}

/// A Lucidos-spawned subprocess proves who it is, and the answer is still
/// no. A call speaks as the owner, and an agent never does, on any
/// authority. Even the home thread's own token is refused.
#[tokio::test]
async fn an_agent_may_not_mint_a_ticket() {
    let (pool, db_name) = setup_test_db().await;
    let line = Line::new();
    voice_set(&pool, true).await;
    let home = the_home_thread(&pool).await;
    init_agent_origin_secret("voice-gate-secret".into());

    for caller in [Some(home), Some(Uuid::new_v4()), None] {
        let token = mint_agent_origin_token(caller, 0, None)
            .expect("the secret is installed, so a token mints");
        let headers = headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, &token)]);
        let error = mint(&pool, &line.sessions, &line.tickets, &headers, home)
            .await
            .err()
            .unwrap_or_else(|| panic!("should refuse an agent from {caller:?}"));
        assert_eq!(error.status, StatusCode::FORBIDDEN);
        assert!(error.message.contains("No agent"), "{}", error.message);
    }

    teardown_test_db(&db_name).await;
}

/// The mint takes the strict origin rule. A browser-shaped request naming an
/// app document is refused, and so is one naming no page at all. The shell's
/// own page gets its ticket.
#[tokio::test]
async fn a_ticket_is_refused_to_an_app_page_and_to_an_unnamed_page() {
    let (pool, db_name) = setup_test_db().await;
    let line = Line::new();
    voice_set(&pool, true).await;
    let home = the_home_thread(&pool).await;
    crate::test_support::seed_device(&pool, OWNERS_PHONE, Some("Mozilla/5.0"), None).await;

    for referer in [Some("http://localhost:3000/app/habit-tracker/"), None] {
        let mut headers = device_headers(OWNERS_PHONE);
        headers.insert("sec-fetch-site", "same-origin".parse().expect("a value"));
        if let Some(referer) = referer {
            headers.insert("referer", referer.parse().expect("a value"));
        }
        let error = mint(&pool, &line.sessions, &line.tickets, &headers, home)
            .await
            .err()
            .unwrap_or_else(|| panic!("should refuse a mint from {referer:?}"));
        assert_eq!(error.status, StatusCode::FORBIDDEN);
        assert_eq!(error.message, NOT_FROM_THE_SHELL);
    }

    let mut shell = device_headers(OWNERS_PHONE);
    shell.insert("sec-fetch-site", "same-origin".parse().expect("a value"));
    shell.insert(
        "referer",
        "http://localhost:3000/".parse().expect("a value"),
    );
    mint(&pool, &line.sessions, &line.tickets, &shell, home)
        .await
        .expect("the shell's own page gets a ticket");

    teardown_test_db(&db_name).await;
}

/// No ticket, or one the mint never issued, opens nothing. Each refusal
/// lands before a slot is claimed.
#[tokio::test]
async fn an_upgrade_without_a_live_ticket_is_refused() {
    let (pool, db_name) = setup_test_db().await;
    let line = Line::new();
    voice_set(&pool, true).await;
    let home = the_home_thread(&pool).await;
    crate::test_support::seed_device(&pool, OWNERS_PHONE, Some("Mozilla/5.0"), None).await;

    for ticket in [None, Some("never-minted")] {
        let error = line
            .dial(&pool, &device_headers(OWNERS_PHONE), home, ticket)
            .await
            .err()
            .unwrap_or_else(|| panic!("should refuse ticket {ticket:?}"));
        assert_eq!(error.status, StatusCode::FORBIDDEN);
        assert!(
            error.message.contains("/api/v1/voice/ticket"),
            "{}",
            error.message
        );
        assert_eq!(line.sessions.count(), 0, "a refusal must claim no slot");
    }

    teardown_test_db(&db_name).await;
}

/// A ticket opens the thread it was minted for, once. Spent on another
/// thread, it is gone, and home refuses it too.
#[tokio::test]
async fn a_ticket_opens_its_own_thread_once() {
    let (pool, db_name) = setup_test_db().await;
    let line = Line::new();
    voice_set(&pool, true).await;
    let home = the_home_thread(&pool).await;
    let other = a_chat_thread(&pool).await;
    let phone = device_headers(OWNERS_PHONE);

    let ticket = line.owner_mints(&pool, home).await.expect("a ticket");
    line.dial(&pool, &phone, other, Some(&ticket))
        .await
        .err()
        .expect("a ticket for home opens no other thread");
    let error = line
        .dial(&pool, &phone, home, Some(&ticket))
        .await
        .err()
        .expect("the cross-thread attempt spent it");
    assert_eq!(error.status, StatusCode::FORBIDDEN);
    assert_eq!(line.sessions.count(), 0, "a refusal must claim no slot");

    let ticket = line.owner_mints(&pool, home).await.expect("a ticket");
    let held = line
        .dial(&pool, &phone, home, Some(&ticket))
        .await
        .expect("a fresh ticket opens home");
    drop(held);
    line.dial(&pool, &phone, home, Some(&ticket))
        .await
        .err()
        .expect("a ticket spends once");

    teardown_test_db(&db_name).await;
}

/// The call speaks as the device that minted the ticket. Direct to the
/// engine there is no header, so the ticket alone names the caller.
#[tokio::test]
async fn the_call_speaks_as_the_device_that_minted_the_ticket() {
    let (pool, db_name) = setup_test_db().await;
    let line = Line::new();
    voice_set(&pool, true).await;
    let home = the_home_thread(&pool).await;

    let ticket = line.owner_mints(&pool, home).await.expect("a ticket");
    let admitted = line
        .dial(&pool, &HeaderMap::new(), home, Some(&ticket))
        .await
        .expect("a call from the owner's phone");
    assert!(
        matches!(&admitted.actor, MessageOrigin::Device { device_id, .. } if device_id == OWNERS_PHONE),
        "the call must speak as the device that minted it: {:?}",
        admitted.actor
    );

    teardown_test_db(&db_name).await;
}

/// Through the gateway the header is the device it authenticated. A ticket
/// another device minted must not stand in for it, even when both are the
/// owner's.
#[tokio::test]
async fn a_header_naming_another_device_than_the_ticket_is_refused() {
    let (pool, db_name) = setup_test_db().await;
    let line = Line::new();
    voice_set(&pool, true).await;
    let home = the_home_thread(&pool).await;
    crate::test_support::seed_device(&pool, OWNERS_LAPTOP, Some("Mozilla/5.0"), None).await;

    let ticket = line.owner_mints(&pool, home).await.expect("a ticket");
    let error = line
        .dial(&pool, &device_headers(OWNERS_LAPTOP), home, Some(&ticket))
        .await
        .err()
        .expect("the header's device did not mint this ticket");
    assert_eq!(error.status, StatusCode::FORBIDDEN);
    assert_eq!(line.sessions.count(), 0, "a refusal must claim no slot");

    teardown_test_db(&db_name).await;
}

/// The upgrade re-checks what the mint checked. Voice switched off between
/// the two refuses the call, and the ticket is spent by the attempt.
#[tokio::test]
async fn the_upgrade_re_checks_what_the_mint_checked() {
    let (pool, db_name) = setup_test_db().await;
    let line = Line::new();
    voice_set(&pool, true).await;
    let home = the_home_thread(&pool).await;
    let phone = device_headers(OWNERS_PHONE);

    let ticket = line.owner_mints(&pool, home).await.expect("a ticket");
    voice_set(&pool, false).await;
    let error = line
        .dial(&pool, &phone, home, Some(&ticket))
        .await
        .err()
        .expect("voice went off after the mint");
    assert!(error.message.contains("under Voice"), "{}", error.message);
    assert_eq!(line.sessions.count(), 0, "a refusal must claim no slot");

    voice_set(&pool, true).await;
    let error = line
        .dial(&pool, &phone, home, Some(&ticket))
        .await
        .err()
        .expect("the refused attempt spent the ticket");
    assert!(
        error.message.contains("/api/v1/voice/ticket"),
        "{}",
        error.message
    );

    teardown_test_db(&db_name).await;
}
