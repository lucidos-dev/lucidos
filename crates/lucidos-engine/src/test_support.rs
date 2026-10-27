//! Shared helpers for unit tests that need a real Postgres connection.
//! Each call to `setup_test_db` creates a fresh database, runs migrations,
//! and returns the pool plus the database name to pass back to `teardown_test_db`.

#![cfg(test)]

use sqlx::postgres::{PgPool, PgPoolOptions};
use std::path::PathBuf;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent, BUS_CAPACITY};
use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};

#[derive(Debug)]
struct ServerError {
    code: &'static str,
    message: String,
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ServerError {}

impl sqlx::error::DatabaseError for ServerError {
    fn message(&self) -> &str {
        &self.message
    }
    fn code(&self) -> Option<std::borrow::Cow<'_, str>> {
        Some(self.code.into())
    }
    fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
        self
    }
    fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + Send + Sync + 'static) {
        self
    }
    fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + Send + Sync + 'static> {
        self
    }
    fn kind(&self) -> sqlx::error::ErrorKind {
        sqlx::error::ErrorKind::Other
    }
}

/// A server error carrying the SQLSTATE `code`, as Postgres would send it.
pub fn server_error(code: &'static str, message: &str) -> sqlx::Error {
    sqlx::Error::Database(Box::new(ServerError {
        code,
        message: message.to_string(),
    }))
}

/// Overrun every subscriber's buffer, so its next `recv` is a
/// `RecvError::Lagged`. The way to reproduce a lag without timing: emit the
/// event under test first, flood, then hand the subscriber's first `recv` to
/// its one-step seam.
///
/// One frame past capacity, so the flood alone lags a subscriber that had
/// nothing buffered. The frame is transient and projection-free, so no
/// subscriber does anything with the ones it still receives.
pub fn flood_past_bus_capacity(bus: &EventBus) {
    for _ in 0..=BUS_CAPACITY {
        bus.broadcast_transient_system(SystemEvent::BackupProgress {
            phase: "flood".into(),
            progress: 0,
            total: 0,
        });
    }
}

fn admin_url() -> String {
    std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://lucidos:lucidos@localhost:5432/postgres".into())
}

/// An `LlmProvider` that hands out one scripted reply per call and reports a
/// fixed token usage. Lets a test drive a retry or resample loop, and assert
/// on the `ContextCaptured` rows each round trip leaves behind.
pub struct ScriptedProvider {
    replies: std::sync::Mutex<std::vec::IntoIter<String>>,
    model: String,
    input_tokens: u32,
    output_tokens: u32,
    requests: std::sync::Mutex<Vec<RecordedRequest>>,
}

/// What one `ScriptedProvider` call was asked: the system prompt and the text
/// of each message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedRequest {
    pub system_prompt: Option<String>,
    pub messages: Vec<String>,
}

impl ScriptedProvider {
    /// One reply per expected call, in order. Running out is an error, which
    /// is what catches a loop calling more times than the test expects.
    pub fn new(model: &str, replies: Vec<&str>) -> Self {
        Self {
            replies: std::sync::Mutex::new(
                replies
                    .into_iter()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
                    .into_iter(),
            ),
            model: model.to_string(),
            input_tokens: 210,
            output_tokens: 4,
            requests: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Every request the provider has received, oldest first.
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().expect("recorded requests").clone()
    }

    pub fn reporting(mut self, input_tokens: u32, output_tokens: u32) -> Self {
        self.input_tokens = input_tokens;
        self.output_tokens = output_tokens;
        self
    }
}

#[async_trait::async_trait]
impl crate::llm::provider::LlmProvider for ScriptedProvider {
    async fn chat(
        &self,
        messages: Vec<crate::llm::provider::Message>,
        _tools: Vec<crate::llm::provider::ToolDefinition>,
        _selection: crate::llm::ModelSelection<'_>,
        system_prompt: Option<&str>,
        _on_token: Option<crate::llm::provider::TokenCallback>,
        _call: crate::llm::metered::CallToken,
    ) -> Result<crate::llm::provider::LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        self.requests
            .lock()
            .expect("recorded requests")
            .push(RecordedRequest {
                system_prompt: system_prompt.map(str::to_string),
                messages: messages
                    .iter()
                    .map(|m| match &m.content {
                        crate::llm::provider::MessageContent::Text(text) => text.clone(),
                        other => format!("{other:?}"),
                    })
                    .collect(),
            });
        let next = self
            .replies
            .lock()
            .expect("scripted replies")
            .next()
            .ok_or("scripted provider ran out of replies")?;
        Ok(crate::llm::provider::LlmResponse {
            content: Some(next),
            tool_calls: vec![],
            stop_reason: Some("end_turn".to_string()),
            output_tokens: Some(self.output_tokens),
            input_tokens: Some(self.input_tokens),
            cache_creation_tokens: None,
            cache_creation_1h_tokens: None,
            cache_creation_30m_tokens: None,
            cache_creation_12h_tokens: None,
            cache_creation_requested_ttl: None,
            stable_prefix_fingerprint: None,
            cache_read_tokens: None,
            served_model: None,
            provider: None,
            thinking_chars: None,
            thinking_blocks: None,
            unknown_sse_dropped: 0,
            model_only_text: None,
            progress_notes: Vec::new(),
            prefill: None,
            service_tier: None,
        })
    }

    fn default_model(&self) -> &str {
        &self.model
    }
}

/// An `LlmProvider` that answers a chat judgment, and keeps every user
/// message and tool list it was sent.
///
/// One reply serves every call. The usage matches [`ScriptedProvider`]'s, so a
/// capture test asserts the same numbers on either.
pub struct JudgmentChatStub {
    reply: Result<serde_json::Value, String>,
    model: String,
    pub sent: std::sync::Mutex<Vec<(String, Vec<crate::llm::provider::ToolDefinition>)>>,
}

impl JudgmentChatStub {
    fn new(reply: Result<serde_json::Value, String>) -> Self {
        Self {
            reply,
            model: crate::core::prefs::MODEL_COMMAND_JUDGE
                .default_text()
                .to_string(),
            sent: std::sync::Mutex::new(vec![]),
        }
    }

    /// Answers through the answer tool with these arguments.
    pub fn answering(arguments: serde_json::Value) -> Self {
        Self::new(Ok(serde_json::json!({
            "content": null,
            "tool_calls": [{
                "id": "call-1",
                "name": crate::llm::judgment::chat::ANSWER_TOOL,
                "arguments": arguments,
            }],
        })))
    }

    /// Ignores the tool and replies in text.
    pub fn replying(text: &str) -> Self {
        Self::new(Ok(serde_json::json!({ "content": text, "tool_calls": [] })))
    }

    /// The call itself fails.
    pub fn failing(error: &str) -> Self {
        Self::new(Err(error.to_string()))
    }

    /// The user messages sent so far.
    pub fn messages(&self) -> Vec<String> {
        self.sent
            .lock()
            .expect("sent")
            .iter()
            .map(|(m, _)| m.clone())
            .collect()
    }
}

#[async_trait::async_trait]
impl crate::llm::provider::LlmProvider for JudgmentChatStub {
    async fn chat(
        &self,
        messages: Vec<crate::llm::provider::Message>,
        tools: Vec<crate::llm::provider::ToolDefinition>,
        _selection: crate::llm::ModelSelection<'_>,
        _system_prompt: Option<&str>,
        _on_token: Option<crate::llm::provider::TokenCallback>,
        _call: crate::llm::metered::CallToken,
    ) -> Result<crate::llm::provider::LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        let text = messages
            .iter()
            .map(|m| match &m.content {
                crate::llm::provider::MessageContent::Text(t) => t.clone(),
                other => format!("{other:?}"),
            })
            .collect::<Vec<_>>()
            .join("\n");
        self.sent.lock().expect("sent").push((text, tools));
        let mut reply = self.reply.clone()?;
        reply["input_tokens"] = 210.into();
        reply["output_tokens"] = 4.into();
        Ok(serde_json::from_value(reply)?)
    }

    fn default_model(&self) -> &str {
        &self.model
    }
}

/// Returns the same 384-dim vector for every text. pgvector stores
/// `vector(384)`, so anything that writes a memory entry needs this width,
/// and every pair of texts is maximally similar.
pub struct Fixed384Embedder;

#[async_trait::async_trait]
impl crate::memory::EmbeddingProvider for Fixed384Embedder {
    async fn embed(
        &self,
        _text: &str,
    ) -> Result<Vec<f32>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(vec![0.1f32; 384])
    }
    async fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(texts.iter().map(|_| vec![0.1f32; 384]).collect())
    }
    fn dimensions(&self) -> usize {
        384
    }
    fn model_id(&self) -> &str {
        "test-fixed-384"
    }
}

/// Every auxiliary capture of one purpose on one thread, oldest first.
pub async fn aux_captures(pool: &PgPool, thread_id: Uuid, purpose: &str) -> Vec<serde_json::Value> {
    sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT payload FROM events \
         WHERE thread_id = $1 AND event_type = 'ContextCaptured' \
           AND payload->>'purpose' = $2 \
         ORDER BY sequence ASC",
    )
    .bind(thread_id)
    .bind(purpose)
    .fetch_all(pool)
    .await
    .expect("read auxiliary captures")
}

/// Connection URL for a throwaway database created by `setup_test_db`. Lets a
/// test open its own dedicated connection to the same DB, as a restarted engine
/// would.
///
/// Swaps the database name, the path segment after the authority, and keeps
/// any query string. A blanket `replace("/postgres", ..)` also rewrites the
/// `//postgres` inside an authority like `postgres://postgres@host:5432/postgres`.
/// The URL then connects as a role named after the throwaway database, which
/// does not exist. That is the canonical local-Postgres URL, so a contributor
/// pointing `TEST_DATABASE_URL` at their own server hits it first.
pub fn test_db_url(db_name: &str) -> String {
    let base = admin_url();
    let (before_query, query) = match base.split_once('?') {
        Some((head, q)) => (head, Some(q)),
        None => (base.as_str(), None),
    };
    // Start looking after the `//` that opens the authority. A URL naming no
    // database then keeps its host, rather than losing everything before the
    // last slash.
    let authority_start = before_query.find("//").map(|i| i + 2).unwrap_or(0);
    let swapped = match before_query[authority_start..].find('/') {
        Some(i) => format!("{}/{}", &before_query[..authority_start + i], db_name),
        None => format!("{}/{}", before_query, db_name),
    };
    match query {
        Some(q) => format!("{}?{}", swapped, q),
        None => swapped,
    }
}

/// The agent role URL unit tests record in place of the one boot provisions.
pub const AGENT_DATABASE_URL: &str =
    "postgresql://lucidos_test_agent:pw@localhost:5432/lucidos_test?options=-c%20default_transaction_read_only%3Don";

/// Record [`AGENT_DATABASE_URL`] as boot would. Every caller records the same
/// value, so the order tests run in does not matter.
pub fn install_agent_database_url() {
    crate::core::set_agent_database_url(AGENT_DATABASE_URL.to_string());
}

pub async fn setup_test_db() -> (PgPool, String) {
    let base_url = admin_url();
    let db_name = format!(
        "lucidos_test_{}",
        Uuid::new_v4().to_string().replace('-', "")
    );
    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&base_url)
        .await
        .expect("admin connect");
    sqlx::query(&format!("CREATE DATABASE \"{}\"", db_name))
        .execute(&admin_pool)
        .await
        .expect("create db");
    admin_pool.close().await;
    let test_url = test_db_url(&db_name);
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&test_url)
        .await
        .expect("connect test db");
    sqlx::migrate!().run(&pool).await.expect("migrations");
    (pool, db_name)
}

/// Emit one thread event as test setup and return the id of the row it wrote.
///
/// **Unwraps both levels of `EventBus::emit`, and that is the whole point.**
/// It returns `Result<Option<EmitResult>>`, so a seed written as
/// `bus.emit(...).await.unwrap();` sails straight past an inner `None`. The
/// fixture is then simply absent, and the first thing that notices is some
/// later `assert_eq!(found.len(), 3)` reporting a 0. A count is a terrible way
/// to learn that the setup never ran, and it sends the next reader hunting
/// through the query instead of the seed, so this fails at the emit and names
/// the event.
///
/// An inner `None` means the variant is transient (`ThreadEvent::is_persisted`)
/// or was deliberately suppressed, rather than that a write was lost, which is
/// its own thing worth being told: a seeding call site wants a row either way.
pub async fn seed_thread_event(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) -> Uuid {
    let event_type = event.event_type();
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap_or_else(|e| panic!("seed {event_type} on thread {thread_id}: {e}"))
    .unwrap_or_else(|| {
        panic!("seed {event_type} on thread {thread_id} was not persisted (emit returned None)")
    })
    .event_id
}

/// Block until this database has `n` backends waiting on a lock. That is the
/// signal that a racing statement reached its lock, rather than merely having
/// been spawned. Scoped to `current_database()`, so a concurrent test's
/// disposable database cannot satisfy it.
///
/// Panics rather than returning on timeout: a race that never materialised
/// means the test proved nothing, and passing quietly is how it would rot.
pub async fn wait_until_blocked_on_locks(pool: &PgPool, n: i64) {
    for _ in 0..200 {
        let blocked: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity \
             WHERE datname = current_database() AND wait_event_type = 'Lock'",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        if blocked >= n {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("fewer than {n} backends ever blocked on a lock, so the race never happened");
}

/// Make Postgres refuse every insert of `event_type` from now on, so an emit
/// fails at Persist exactly as a real store error would. One per database.
pub async fn refuse_event_type(pool: &PgPool, event_type: &str) {
    sqlx::query(&format!(
        "CREATE FUNCTION refuse_event() RETURNS trigger AS $$ BEGIN \
         IF NEW.event_type = '{event_type}' THEN RAISE EXCEPTION 'refused'; END IF; \
         RETURN NEW; END $$ LANGUAGE plpgsql"
    ))
    .execute(pool)
    .await
    .expect("create the refusing function");
    sqlx::query(
        "CREATE TRIGGER refuse_event BEFORE INSERT ON events \
         FOR EACH ROW EXECUTE FUNCTION refuse_event()",
    )
    .execute(pool)
    .await
    .expect("create the refusing trigger");
}

/// A new chat thread opened by a human message, so it has a
/// `thread_summaries` row for later events to project into.
pub async fn seed_chat_thread(bus: &EventBus, text: &str) -> Uuid {
    let thread_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: message_received(text),
        meta: EventMeta {
            channel: Some(crate::engine::thread_events::EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap_or_else(|e| panic!("seed a chat thread: {e}"));
    thread_id
}

/// A person's message, with every optional field empty.
pub fn message_received(text: &str) -> ThreadEvent {
    ThreadEvent::MessageReceived {
        provider: None,
        voice_session_id: None,
        wake_mode: None,
        text: text.into(),
        user_image_hashes: vec![],
        device_id: None,
        image_description: None,
        parent_thread_id: None,
        spawning_event_id: None,
        mode: crate::engine::thread_events::ActorMode::Human,
        model: None,
        reasoning_effort: None,
        origin: None,
    }
}

/// One of each side-question event, for the tests pinning that no agent,
/// trigger or event wait ever sees one (ADR 0320).
pub fn every_side_question_event() -> Vec<ThreadEvent> {
    let side_question_id = Uuid::new_v4();
    vec![
        ThreadEvent::SideQuestionAsked {
            side_question_id,
            question: "what does this return?".into(),
            image_hashes: vec![],
        },
        ThreadEvent::SideQuestionAnswered {
            side_question_id,
            answer: "A string.".into(),
        },
        ThreadEvent::SideQuestionFailed {
            side_question_id,
            error: "no answer".into(),
        },
        ThreadEvent::SideQuestionDismissed { side_question_id },
    ]
}

/// Build a tiny git repo with `main` + initial commit and a worktree on
/// `branch`. Returns `(tmpdir, repo_root, worktree_path)`. Caller must keep
/// `tmpdir` in scope for the test duration: dropping it `rm -rf`'s the repo.
///
/// Shared by tests that need a real on-disk git repo + worktree pair (the
/// `seed_branch_work` tests in `session_seed_tests.rs` and the startup
/// sweep tests in `agent_recovery_tests.rs`). The same boilerplate also lives
/// in git_ops_tests.rs / worktree_cleanup_tests.rs.
pub async fn make_repo_and_worktree(branch: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();

    use crate::engine::git_ops::git_cmd;
    git_cmd(&["init", "-b", "main"], &repo).await.unwrap();
    git_cmd(&["config", "user.email", "test@example.com"], &repo)
        .await
        .unwrap();
    git_cmd(&["config", "user.name", "Test"], &repo)
        .await
        .unwrap();
    std::fs::write(repo.join("seed.txt"), "x").unwrap();
    git_cmd(&["add", "."], &repo).await.unwrap();
    git_cmd(&["commit", "-m", "init"], &repo).await.unwrap();

    let wt = tmp.path().join("wt");
    git_cmd(
        &["worktree", "add", wt.to_str().unwrap(), "-b", branch],
        &repo,
    )
    .await
    .unwrap();

    (tmp, repo, wt)
}

/// Read one thread's projected change state, reason and restart flag included.
/// Panics if the row is missing: every caller seeds a `SessionStarted` first,
/// so a missing row means the projection write did not land.
pub async fn read_change_state(
    pool: &PgPool,
    thread_id: Uuid,
) -> crate::engine::thread_lifecycle::CodingAgentChangeState {
    let (kind, reason, requires_restart) = sqlx::query_as::<_, (String, Option<String>, bool)>(
        "SELECT coding_agent_change_state, coding_agent_unproposed_reason, \
                coding_agent_requires_restart \
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap();
    crate::engine::thread_lifecycle::CodingAgentChangeState::from_columns(
        &kind,
        reason.as_deref(),
        requires_restart,
    )
}

/// The coarse half of [`read_change_state`].
pub async fn read_change_state_kind(
    pool: &PgPool,
    thread_id: Uuid,
) -> crate::engine::thread_lifecycle::ChangeStateKind {
    read_change_state(pool, thread_id).await.kind()
}

/// Set one thread's change state directly, as a fixture. The `CHECK`
/// constraints still apply, so an illegal state fails the test.
pub async fn seed_change_state(
    pool: &PgPool,
    thread_id: Uuid,
    state: crate::engine::thread_lifecycle::CodingAgentChangeState,
) {
    use crate::engine::thread_lifecycle::CodingAgentChangeState as S;
    let (reason, requires_restart) = match state {
        S::None => (None, false),
        S::Unproposed { reason } => (reason.map(|r| r.as_str()), false),
        S::Proposed { requires_restart } => (None, requires_restart),
    };
    sqlx::query(
        "UPDATE thread_summaries SET coding_agent_change_state = $2, \
                coding_agent_unproposed_reason = $3, coding_agent_requires_restart = $4 \
         WHERE thread_id = $1",
    )
    .bind(thread_id)
    .bind(state.kind().as_str())
    .bind(reason)
    .bind(requires_restart)
    .execute(pool)
    .await
    .unwrap();
}

/// Emit `SessionStarted` on the `CodingAgent` channel so the projection
/// upserts an `is_coding_agent=true, state='active'` row in `thread_summaries`. Pass
/// `repo_id = Some(...)` to stamp `cc_repo_id` on the projection row (the
/// external-repo branch); `None` covers the in-workspace common case.
///
/// `session_id` is a fixed sentinel — no test reads it back. The branch and
/// repo_id are the per-test inputs that actually matter.
pub async fn start_cc_session(
    bus: &EventBus,
    thread_id: Uuid,
    branch: &str,
    repo_id: Option<String>,
) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "test-session".into(),
            branch: branch.into(),
            repo_id,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

/// A coding-agent thread with a session, a child of `parent` when given.
pub async fn seed_coding_thread(bus: &EventBus, parent: Option<Uuid>) -> Uuid {
    let thread_id = Uuid::new_v4();
    emit_cc(bus, thread_id, message_from_parent("work", parent)).await;
    start_cc_session(bus, thread_id, &format!("claude-code/{thread_id}"), None).await;
    thread_id
}

/// A legacy *successor thread* chain: `old` continued in a new thread, which
/// opens as a child of `parent` when given. Nothing creates one any more, but
/// their events stay and must keep reading as they did. Returns the successor.
pub async fn seed_handoff(bus: &EventBus, old: Uuid, parent: Option<Uuid>) -> Uuid {
    let successor = Uuid::new_v4();
    emit_cc(
        bus,
        old,
        ThreadEvent::SuccessorThreadSpawned {
            successor_thread_id: successor,
            base_sha: "0123abc".into(),
            starts_build_chain: false,
            handoff_consent: None,
        },
    )
    .await;
    emit_cc(bus, successor, message_from_parent("carry on", parent)).await;
    successor
}

fn message_from_parent(text: &str, parent: Option<Uuid>) -> ThreadEvent {
    let mut message = message_received(text);
    if let ThreadEvent::MessageReceived {
        parent_thread_id, ..
    } = &mut message
    {
        *parent_thread_id = parent;
    }
    message
}

async fn emit_cc(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap_or_else(|e| panic!("emit on {thread_id}: {e}"));
}

/// An `EventWaitStarted` armed now, watching `BackgroundBashCompleted` for an
/// hour: the wait a coding agent's `lucidos background-task run` arms.
pub fn event_wait_started(wait_id: Uuid) -> ThreadEvent {
    ThreadEvent::EventWaitStarted {
        wait_id,
        tool_use_id: format!("toolu_{}", wait_id.simple()),
        on: vec![crate::core::event_subscription::EventSubscription {
            event_type: "BackgroundBashCompleted".into(),
            condition: None,
        }],
        reason: "the e2e suite to finish".into(),
        armed_at: chrono::Utc::now(),
        expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
        watermark: 0,
    }
}

/// Seed a credential for a test that only cares that one exists.
///
/// The store's mutators take the `EventBus` because a credential write and its
/// `Credential{Created,Updated}` announcement are one operation (see
/// `CredentialStore`'s type doc). A fixture has no bus of its own and no
/// interest in the event, so this builds a throwaway one against the test
/// database rather than pushing that ceremony into every seeding call site.
/// Tests that assert ON the event build their own bus and call the store
/// directly.
pub async fn seed_credential(
    pool: &PgPool,
    service_name: &str,
    base_url: &str,
    auth_type: crate::core::AuthType,
    auth_value: &str,
) {
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    crate::core::CredentialStore::upsert(
        pool,
        &bus,
        service_name,
        &[base_url.to_string()],
        auth_type,
        auth_value,
        None,
        None,
        None,
    )
    .await
    .unwrap_or_else(|e| panic!("seed credential {service_name}: {e}"));
}

/// Delete a credential in a test.
///
/// Exists so callers outside the engine layer do not have to name
/// `crate::engine::event_bus` to build a bus for the store's
/// `CredentialDeleted` emit: `llm/*.rs` is forbidden from depending on
/// `crate::engine` (see `llm::validate::tests::llm_does_not_depend_on_engine`).
/// Still takes a NAME rather than the store's id, because that is what a
/// fixture has in hand. It resolves the id first, so a caller does not have to
/// thread one through just to clean up after itself.
pub async fn delete_credential(pool: &PgPool, service_name: &str) {
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let Some(id) = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT id FROM credentials WHERE service_name = $1 LIMIT 1",
    )
    .bind(service_name)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| panic!("look up credential {service_name}: {e}")) else {
        return;
    };
    crate::core::CredentialStore::delete(pool, &bus, id, None)
        .await
        .unwrap_or_else(|e| panic!("delete credential {service_name}: {e}"));
}

/// Register a device and give it a display name, for a test that needs one to
/// exist (actor resolution, presence, device-scoped preferences).
///
/// Same rationale as [`seed_credential`]: the store's mutators own their
/// `Device{Registered,Renamed}` emits, and a fixture has no bus of its own.
pub async fn seed_device(pool: &PgPool, id: &str, user_agent: Option<&str>, name: Option<&str>) {
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    crate::core::DeviceStore::register(pool, &bus, id, user_agent, None, None)
        .await
        .unwrap_or_else(|e| panic!("seed device {id}: {e}"));
    if name.is_some() {
        crate::core::DeviceStore::rename(pool, &bus, id, name, None)
            .await
            .unwrap_or_else(|e| panic!("name device {id}: {e}"));
    }
}

/// Seed a connected OAuth account for a test that needs one to exist (token
/// resolution, provider routing, scope merging).
///
/// Argument order mirrors `OAuthStore::connect` minus the bus and actor, which
/// a fixture has no opinion about. Same rationale as [`seed_credential`].
#[allow(clippy::too_many_arguments)] // mirrors the store's column list
pub async fn seed_oauth_account(
    pool: &PgPool,
    provider: &str,
    email: Option<&str>,
    display_name: Option<&str>,
    access_token: &str,
    refresh_token: Option<&str>,
    token_expiry: Option<chrono::DateTime<chrono::Utc>>,
    scopes: &str,
) -> Result<uuid::Uuid, sqlx::Error> {
    seed_oauth_account_with_desired(
        pool,
        provider,
        email,
        display_name,
        access_token,
        refresh_token,
        token_expiry,
        scopes,
        scopes,
    )
    .await
}

/// Seed an account whose GRANTED scopes differ from the set it was ASKED for.
///
/// The shape a provider produces when it refuses part of a request (a Dropbox
/// app whose Permissions tab has not enabled a scope). [`seed_oauth_account`]
/// cannot express it: it seeds granted and desired identically, which is the
/// ordinary case and says nothing about recovery.
#[allow(clippy::too_many_arguments)] // mirrors the store's column list
pub async fn seed_oauth_account_with_desired(
    pool: &PgPool,
    provider: &str,
    email: Option<&str>,
    display_name: Option<&str>,
    access_token: &str,
    refresh_token: Option<&str>,
    token_expiry: Option<chrono::DateTime<chrono::Utc>>,
    scopes: &str,
    desired_scopes: &str,
) -> Result<uuid::Uuid, sqlx::Error> {
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    crate::core::OAuthStore::connect(
        pool,
        &bus,
        provider,
        email,
        display_name,
        access_token,
        refresh_token,
        token_expiry,
        scopes,
        desired_scopes,
        None,
    )
    .await
    .map(|stored| stored.id)
}

/// Seed a global preference for a test that just needs one stored.
///
/// Same rationale as [`seed_credential`]: `PreferenceStore`'s writers announce
/// `PreferencesChanged`, and a fixture has no bus of its own. Returns the
/// store's `Result` so existing call sites keep their `.unwrap()`. Tests that
/// assert ON the announcement build a bus and call the store directly.
pub async fn seed_preference(pool: &PgPool, key: &str, value: &str) -> Result<(), sqlx::Error> {
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    crate::core::PreferenceStore::set(pool, &bus, key, value, None).await
}

/// Store a Claude gateway sign-in from `issuer`, through its store.
pub async fn seed_claude_gateway_sign_in(pool: &PgPool, issuer: &str) {
    use crate::llm::claude_gateway::{account::ClaudeGatewayAccountStore, GatewayOrigin};
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let issuer = GatewayOrigin::parse(issuer).expect("a gateway origin");
    let tokens = crate::llm::claude_gateway::GrantedTokens {
        access_token: "gw-token".to_string(),
        refresh_token: None,
        token_expiry: None,
        email: None,
    };
    ClaudeGatewayAccountStore::sign_in(pool, &bus, &issuer, &tokens, None)
        .await
        .expect("store the sign-in");
}

/// [`seed_preference`], scoped to one device.
pub async fn seed_preference_for_device(
    pool: &PgPool,
    key: &str,
    value: &str,
    device_id: &str,
) -> Result<(), sqlx::Error> {
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    crate::core::PreferenceStore::set_for_device(pool, &bus, key, value, device_id, None).await
}

/// Insert one row on a trigger's aggregate stream, back-dated to `created`.
///
/// A raw insert rather than an emit, because the run-history reads compare
/// against `created`, and the bus always stamps the database's `now()`.
pub async fn seed_trigger_event(
    pool: &PgPool,
    trigger_id: &str,
    event_type: &str,
    created: &str,
    payload: serde_json::Value,
) {
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, aggregate, aggregate_id, created) \
         VALUES ($1, $2, $3, 'trigger', $4, $5::timestamptz)",
    )
    .bind(Uuid::new_v4())
    .bind(event_type)
    .bind(payload)
    .bind(trigger_id)
    .bind(created)
    .execute(pool)
    .await
    .expect("insert trigger event");
}

/// An `EventBus` backed by a pool that never dials, for tests whose subject is
/// a rejection or a filesystem effect and that never reach an emit.
///
/// `connect_lazy` builds the pool without touching the network, so this stays a
/// fast no-DB test. If a test using it DOES reach an emit, `emit_or_log` logs
/// the connection failure rather than panicking, which is the wrong shape for
/// an assertion: a test that cares about the event wants a real
/// `setup_test_db`.
///
/// The short acquire timeout is what keeps "reaches an emit" cheap. sqlx retries
/// a refused connection until the timeout expires, so on the 30s default a
/// single unwanted emit stalls the test for half a minute (three writes in one
/// test, and it is a 90s test).
pub fn offline_event_bus() -> EventBus {
    let pool = PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(50))
        .connect_lazy("postgres://lucidos:lucidos@127.0.0.1:1/offline")
        .expect("lazy pool");
    let (bus, _callback_rx) = EventBus::new(pool);
    bus
}

pub async fn teardown_test_db(db_name: &str) {
    let base_url = admin_url();
    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&base_url)
        .await
        .expect("admin connect");
    let _ = sqlx::query(&format!(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{}'",
        db_name
    ))
    .execute(&admin_pool)
    .await;
    let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS \"{}\"", db_name))
        .execute(&admin_pool)
        .await;
    admin_pool.close().await;
}

/// Reading the engine's own sources, for the repo's source-scan tripwires
/// (`announced_surfaces_tests`, the trigger write chokepoint guard).
///
/// Rust is not parsed. These helpers only locate the *production* text of each
/// file so a scan can look for a structure the codebase forbids. Test modules
/// are excluded by path convention and each file is truncated at its first
/// top-level `#[cfg(test)]`, because a fixture doing the forbidden thing is
/// setup rather than a real call site.
pub mod source_scan {
    use std::path::{Path, PathBuf};

    /// `crates/lucidos-engine/src`.
    pub fn src_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
    }

    /// A path convention test module.
    pub fn is_test_path(rel: &str) -> bool {
        rel.split('/').any(|part| {
            let base = part.strip_suffix(".rs").unwrap_or(part);
            base == "tests" || base == "bin" || base.ends_with("_test") || base.ends_with("_tests")
        })
    }

    /// Read a source file with its inline test module cut off. Inline
    /// `mod tests` sits at the end of the file by convention, so truncating at
    /// the first top-level `#[cfg(test)]` drops it whole. A file-level
    /// `#![cfg(test)]` never compiles outside tests, so it has no production
    /// text at all.
    pub fn read_production_source(path: &Path) -> String {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        if text.lines().any(|line| line.trim() == "#![cfg(test)]") {
            return String::new();
        }
        match text.find("\n#[cfg(test)]") {
            Some(idx) => text[..idx].to_string(),
            None => text,
        }
    }

    /// Every non-test engine source, as `(path relative to src/, production text)`.
    pub fn production_sources() -> Vec<(String, String)> {
        let root = src_root();
        let mut out = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read_dir").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let rel = path
                    .strip_prefix(&root)
                    .expect("under src")
                    .to_string_lossy()
                    .replace('\\', "/");
                if is_test_path(&rel) {
                    continue;
                }
                let text = read_production_source(&path);
                out.push((rel, text));
            }
        }
        out.sort();
        out
    }
}
