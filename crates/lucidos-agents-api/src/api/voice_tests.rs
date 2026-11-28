use super::*;
use crate::api::socket_ticket::SocketTickets;
use crate::api::voice_ticket::{mint, NOT_FROM_THE_SHELL};
use crate::core::subprocess_origin::{
    init_agent_origin_secret, mint_agent_origin_token, HEADER_AGENT_ORIGIN_TOKEN,
    QUERY_SOCKET_TICKET,
};
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
    crate::api::gateway_stamp::test_support::stamped_device_headers(device_id)
}

fn call_uri(thread_id: Uuid, ticket: Option<&str>) -> Uri {
    let ticket = ticket.map_or(String::new(), |t| format!("&{QUERY_SOCKET_TICKET}={t}"));
    format!("/voice?thread_id={thread_id}{ticket}")
        .parse()
        .expect("a valid URI")
}

struct Line {
    sessions: LiveVoiceSessions,
    tickets: SocketTickets,
}

impl Line {
    fn new() -> Self {
        Self {
            sessions: LiveVoiceSessions::new(),
            tickets: SocketTickets::new(),
        }
    }

    async fn owner_mints(&self, pool: &sqlx::PgPool, thread_id: Uuid) -> Result<String, ApiError> {
        crate::test_support::seed_device(pool, OWNERS_PHONE, Some("Mozilla/5.0"), None).await;
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        mint(
            pool,
            &bus,
            &self.sessions,
            &self.tickets,
            &device_headers(OWNERS_PHONE),
            thread_id,
        )
        .await
    }

    async fn dial(
        &self,
        pool: &sqlx::PgPool,
        headers: &HeaderMap,
        thread_id: Uuid,
        ticket: Option<&str>,
    ) -> Result<Admitted, ApiError> {
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        admit(
            pool,
            &bus,
            &self.sessions,
            &self.tickets,
            headers,
            &call_uri(thread_id, ticket),
            thread_id,
        )
        .await
    }

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

async fn the_home_thread(pool: &sqlx::PgPool) -> Uuid {
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    crate::engine::home_thread::ensure_home_thread(&bus, pool)
        .await
        .expect("create the home thread")
}

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

async fn voice_set(pool: &sqlx::PgPool, on: bool) {
    let key = crate::core::prefs::VOICE_ENABLED.key();
    crate::core::PreferenceStore::set_row_for_test(pool, key, if on { "true" } else { "false" })
        .await
        .expect("set the voice switch");
}

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

    voice_set(&pool, true).await;
    line.owner_calls(&pool, thread_id)
        .await
        .expect("a call once voice is on");

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_unreadable_voice_switch_refuses_the_call() {
    let (pool, db_name) = setup_test_db().await;
    voice_set(&pool, true).await;
    let thread_id = the_home_thread(&pool).await;
    let sessions = LiveVoiceSessions::new();
    pool.close().await;

    let error = refuse_call_on(&pool, &sessions, thread_id)
        .await
        .expect_err("an unreadable switch must read as off");
    assert_eq!(error.status, StatusCode::FORBIDDEN);
    assert!(error.message.contains("under Voice"), "{}", error.message);

    teardown_test_db(&db_name).await;
}

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

    drop(held);
    let second_id = line
        .owner_calls(&pool, thread_id)
        .await
        .expect("next call")
        .session_id;
    assert_ne!(first_id, second_id);

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_caller_naming_nobody_gets_no_ticket() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    let line = Line::new();
    voice_set(&pool, true).await;
    let home = the_home_thread(&pool).await;

    let mut typed = HeaderMap::new();
    typed.insert(
        crate::core::subprocess_origin::HEADER_DEVICE_ID,
        "nobody-registered".parse().unwrap(),
    );
    for headers in [HeaderMap::new(), typed] {
        let error = mint(&pool, &bus, &line.sessions, &line.tickets, &headers, home)
            .await
            .expect_err("should refuse an unidentified caller");
        assert_eq!(error.status, StatusCode::UNAUTHORIZED);
    }

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_agent_may_not_mint_a_ticket() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    let line = Line::new();
    voice_set(&pool, true).await;
    let home = the_home_thread(&pool).await;
    init_agent_origin_secret("voice-gate-secret".into());

    for caller in [Some(home), Some(Uuid::new_v4()), None] {
        let token = mint_agent_origin_token(caller, 0, None)
            .expect("the secret is installed, so a token mints");
        let headers = headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, &token)]);
        let error = mint(&pool, &bus, &line.sessions, &line.tickets, &headers, home)
            .await
            .err()
            .unwrap_or_else(|| panic!("should refuse an agent from {caller:?}"));
        assert_eq!(error.status, StatusCode::FORBIDDEN);
        assert!(error.message.contains("No agent"), "{}", error.message);
    }

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_ticket_is_refused_to_an_app_page_and_to_an_unnamed_page() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
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
        let error = mint(&pool, &bus, &line.sessions, &line.tickets, &headers, home)
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
    mint(&pool, &bus, &line.sessions, &line.tickets, &shell, home)
        .await
        .expect("the shell's own page gets a ticket");

    teardown_test_db(&db_name).await;
}

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

#[tokio::test]
async fn the_call_speaks_as_the_device_that_minted_the_ticket() {
    let (pool, db_name) = setup_test_db().await;
    let line = Line::new();
    voice_set(&pool, true).await;
    let home = the_home_thread(&pool).await;

    let ticket = line.owner_mints(&pool, home).await.expect("a ticket");
    let admitted = line
        .dial(&pool, &device_headers(OWNERS_PHONE), home, Some(&ticket))
        .await
        .expect("a call from the owner's phone");
    assert!(
        matches!(&admitted.actor, MessageOrigin::Device { device_id, .. } if device_id == OWNERS_PHONE),
        "the call must speak as the device that minted it: {:?}",
        admitted.actor
    );

    teardown_test_db(&db_name).await;
}

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

#[tokio::test]
async fn restore_refuses_old_voice_ticket_and_accepts_a_new_one() {
    let (pool, db) = setup_test_db().await;
    voice_set(&pool, true).await;
    let line = Line::new();
    let home = the_home_thread(&pool).await;
    let old = line.owner_mints(&pool, home).await.unwrap();
    let generation = Uuid::new_v4();
    sqlx::query("INSERT INTO snapshot_restore_boundary (singleton, restore_id, snapshot, restored_at, event_sequence) VALUES (TRUE, $1, 'fixture', NOW(), 0)")
        .bind(generation).execute(&pool).await.unwrap();
    assert!(line
        .dial(&pool, &device_headers(OWNERS_PHONE), home, Some(&old))
        .await
        .is_err());
    let current = line.owner_mints(&pool, home).await.unwrap();
    let admitted = line
        .dial(&pool, &device_headers(OWNERS_PHONE), home, Some(&current))
        .await
        .unwrap();
    drop(admitted);
    pool.close().await;
    teardown_test_db(&db).await;
}
