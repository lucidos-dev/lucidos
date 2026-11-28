#![cfg(any(test, feature = "test-support"))]

/// INVARIANT: Inbox query tests share the production decoder slot across test databases.
pub static DIAGNOSTIC_QUERY_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

use crate::core::store::MIGRATOR;
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::Connection;
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

pub fn server_error(code: &'static str, message: &str) -> sqlx::Error {
    sqlx::Error::Database(Box::new(ServerError {
        code,
        message: message.to_string(),
    }))
}

/// INVARIANT: Overrun every subscriber's buffer, so its next `recv` is a
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

pub struct ScriptedProvider {
    replies: std::sync::Mutex<std::vec::IntoIter<String>>,
    model: String,
    input_tokens: u32,
    output_tokens: u32,
    requests: std::sync::Mutex<Vec<RecordedRequest>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedRequest {
    pub system_prompt: Option<String>,
    pub messages: Vec<String>,
}

impl ScriptedProvider {
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
            usage: Some(crate::llm::api_usage::ApiUsage {
                input_tokens: self.input_tokens,
                output_tokens: self.output_tokens,
                ..Default::default()
            }),
            stable_prefix_fingerprint: None,
            served_model: None,
            provider: None,
            thinking_chars: None,
            thinking_blocks: None,
            unknown_sse_dropped: 0,
            model_only_text: None,
            progress_notes: Vec::new(),
            prefill: None,
        })
    }

    fn default_model(&self) -> &str {
        &self.model
    }
}

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

    pub fn served_by(mut self, model: &str) -> Self {
        if let Ok(reply) = self.reply.as_mut() {
            reply["served_model"] = model.into();
        }
        self
    }

    pub fn replying(text: &str) -> Self {
        Self::new(Ok(serde_json::json!({ "content": text, "tool_calls": [] })))
    }

    pub fn failing(error: &str) -> Self {
        Self::new(Err(error.to_string()))
    }

    pub fn writing_the_1h_cache(mut self, tokens: u32) -> Self {
        if let Ok(reply) = &mut self.reply {
            reply["usage"]["cache_creation_tokens"] = tokens.into();
            reply["usage"]["cache_creation_1h_tokens"] = tokens.into();
        }
        self
    }

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
        let usage = &mut reply["usage"];
        usage["input_tokens"] = 210.into();
        usage["output_tokens"] = 4.into();
        for count in ["cache_read_tokens", "cache_creation_tokens"] {
            if usage[count].is_null() {
                usage[count] = 0.into();
            }
        }
        Ok(serde_json::from_value(reply)?)
    }

    fn default_model(&self) -> &str {
        &self.model
    }
}

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

/// INVARIANT: Connection URL for a throwaway database created by `setup_test_db`. Lets a
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

pub const AGENT_DATABASE_URL: &str =
    "postgresql://lucidos_test_agent:pw@localhost:5432/lucidos_test?options=-c%20default_transaction_read_only%3Don";

/// INVARIANT: Record [`AGENT_DATABASE_URL`] as boot would. The first record wins, and a
/// test that builds a whole engine records the role it provisioned. So a test
/// reads the recorded URL, never this constant.
pub fn install_agent_database_url() {
    crate::core::set_agent_database_url(AGENT_DATABASE_URL.to_string());
}

pub async fn setup_test_db() -> (PgPool, String) {
    let db_name = format!(
        "lucidos_test_{}",
        Uuid::new_v4().to_string().replace('-', "")
    );
    clone_template_db(&template_db_name(&MIGRATOR), &db_name).await;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&test_db_url(&db_name))
        .await
        .expect("connect test db");
    (pool, db_name)
}

/// INVARIANT: The template a migration set clones from. The hash covers every migration's
/// version and checksum, so an edited or added migration names a new template.
/// The prefix stays outside `lucidos_test_%`, which `test-engine.sh` sweeps.
fn template_db_name(migrator: &sqlx::migrate::Migrator) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for migration in migrator.iter() {
        hasher.update(migration.version.to_le_bytes());
        hasher.update(&migration.checksum);
    }
    let digest = format!("{:x}", hasher.finalize());
    format!("lucidos_template_{}", &digest[..16])
}

const STALE_TEMPLATE_SECS: i64 = 7 * 24 * 3600;

/// INVARIANT: Clone `template` into `db_name`, building the template first if needed.
///
/// Locking, on `hashtext(template)` as a session advisory lock: a clone holds
/// it shared, a build or a stale-template drop holds it exclusive. So a
/// template never disappears or half-exists under a clone, across worktrees.
async fn clone_template_db(template: &str, db_name: &str) {
    let mut admin = sqlx::PgConnection::connect(&admin_url())
        .await
        .expect("admin connect");
    loop {
        sqlx::query("SELECT pg_advisory_lock_shared(hashtext($1))")
            .bind(template)
            .execute(&mut admin)
            .await
            .expect("take template lock");
        let ready = template_is_ready(&mut admin, template).await;
        if ready {
            create_from_template(&mut admin, template, db_name).await;
        }
        sqlx::query("SELECT pg_advisory_unlock_shared(hashtext($1))")
            .bind(template)
            .execute(&mut admin)
            .await
            .expect("release template lock");
        if ready {
            break;
        }
        build_template_db(&mut admin, template).await;
    }
    admin.close().await.ok();
}

async fn template_is_ready(admin: &mut sqlx::PgConnection, template: &str) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT datistemplate FROM pg_database WHERE datname = $1")
        .bind(template)
        .fetch_optional(admin)
        .await
        .expect("read template state")
        .unwrap_or(false)
}

async fn create_from_template(admin: &mut sqlx::PgConnection, template: &str, db_name: &str) {
    let server_version: i32 =
        sqlx::query_scalar("SELECT current_setting('server_version_num')::int")
            .fetch_one(&mut *admin)
            .await
            .expect("read server version");
    let strategy = if server_version >= 150_000 {
        " STRATEGY FILE_COPY"
    } else {
        ""
    };
    let sql = format!("CREATE DATABASE \"{db_name}\" TEMPLATE \"{template}\"{strategy}");
    // INVARIANT: Postgres refuses a clone while any session is connected to the template
    // (55006). Nobody should be, since it disallows connections, but a backend
    // still closing can linger for a moment.
    for attempt in 1.. {
        match sqlx::query(&sql).execute(&mut *admin).await {
            Ok(_) => return,
            Err(sqlx::Error::Database(e))
                if e.code().as_deref() == Some("55006") && attempt < 20 =>
            {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Err(e) => panic!("clone {template} into {db_name}: {e}"),
        }
    }
}

/// INVARIANT: Build `template` with the same migrations a fresh database used to run, then
/// mark it a template that refuses connections. The mark is the ready signal,
/// so a build that died half-way is dropped and redone.
async fn build_template_db(admin: &mut sqlx::PgConnection, template: &str) {
    sqlx::query("SELECT pg_advisory_lock(hashtext($1))")
        .bind(template)
        .execute(&mut *admin)
        .await
        .expect("take template build lock");
    if !template_is_ready(admin, template).await {
        sqlx::query(&format!("DROP DATABASE IF EXISTS \"{template}\""))
            .execute(&mut *admin)
            .await
            .expect("drop half-built template");
        sqlx::query(&format!("CREATE DATABASE \"{template}\""))
            .execute(&mut *admin)
            .await
            .expect("create template");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&test_db_url(template))
            .await
            .expect("connect template");
        MIGRATOR.run(&pool).await.expect("migrations");
        pool.close().await;
        sqlx::query(&format!(
            "COMMENT ON DATABASE \"{template}\" IS '{}'",
            chrono::Utc::now().timestamp()
        ))
        .execute(&mut *admin)
        .await
        .expect("stamp template");
        sqlx::query(&format!(
            "ALTER DATABASE \"{template}\" WITH IS_TEMPLATE true ALLOW_CONNECTIONS false"
        ))
        .execute(&mut *admin)
        .await
        .expect("mark template ready");
        drop_stale_templates(admin, template).await;
    }
    sqlx::query("SELECT pg_advisory_unlock(hashtext($1))")
        .bind(template)
        .execute(&mut *admin)
        .await
        .expect("release template build lock");
}

async fn drop_stale_templates(admin: &mut sqlx::PgConnection, keep: &str) {
    for name in stale_templates(admin, keep, None).await {
        drop_if_still_stale(admin, keep, &name).await;
    }
}

async fn stale_templates(
    admin: &mut sqlx::PgConnection,
    keep: &str,
    only: Option<&str>,
) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT datname FROM (
           SELECT d.datname, shobj_description(d.oid, 'pg_database') AS built
           FROM pg_database d
           WHERE d.datname LIKE 'lucidos\\_template\\_%' AND d.datname <> $1
             AND ($3::text IS NULL OR d.datname = $3)
         ) t
         WHERE CASE WHEN built ~ '^[0-9]+$' THEN built::bigint ELSE 0 END
               < extract(epoch FROM now())::bigint - $2",
    )
    .bind(keep)
    .bind(STALE_TEMPLATE_SECS)
    .bind(only)
    .fetch_all(&mut *admin)
    .await
    .expect("list stale templates")
}

async fn drop_if_still_stale(admin: &mut sqlx::PgConnection, keep: &str, name: &str) {
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtext($1))")
        .bind(name)
        .fetch_one(&mut *admin)
        .await
        .expect("try stale template lock");
    if !locked {
        return;
    }
    if !stale_templates(admin, keep, Some(name)).await.is_empty() {
        let _ = sqlx::query(&format!("ALTER DATABASE \"{name}\" WITH IS_TEMPLATE false"))
            .execute(&mut *admin)
            .await;
        let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS \"{name}\""))
            .execute(&mut *admin)
            .await;
    }
    sqlx::query("SELECT pg_advisory_unlock(hashtext($1))")
        .bind(name)
        .execute(&mut *admin)
        .await
        .expect("release stale template lock");
}

/// INVARIANT: Emit one thread event as test setup and return the id of the row it wrote.
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

pub async fn seed_pre_gate_card(pool: &PgPool, thread_id: Uuid, event: ThreadEvent) {
    let event_type = event.event_type();
    let payload = event.to_payload(&EventMeta::NONE);
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, thread_id) \
         VALUES ($1, 'thread', $2, $3, $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(thread_id.to_string())
    .bind(event_type)
    .bind(payload)
    .bind(thread_id)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("insert pre-gate {event_type} on thread {thread_id}: {e}"));
}

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

pub async fn make_repo_and_worktree(branch: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();

    use crate::core::git::git_cmd;
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

pub async fn read_change_state_kind(
    pool: &PgPool,
    thread_id: Uuid,
) -> crate::engine::thread_lifecycle::ChangeStateKind {
    read_change_state(pool, thread_id).await.kind()
}

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

pub async fn start_cc_session(
    bus: &EventBus,
    thread_id: Uuid,
    branch: &str,
    repo_id: Option<String>,
) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
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

pub async fn seed_pending_change(bus: &EventBus, thread_id: Uuid, branch: &str) -> Uuid {
    start_cc_session(bus, thread_id, branch, None).await;
    let change_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeProposed {
            content: Default::default(),
            change_id: change_id.to_string(),
            description: Some("Fix".into()),
            files: vec!["src/main.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: branch.into(),
            repo_root: "/tmp".into(),
            hardened: false,
            incomplete: false,
            set_aside: false,
            path: String::new(),
            diff: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    change_id
}

pub async fn in_engine_child(test: &str) -> bool {
    const CHILD: &str = "LUCIDOS_TEST_ENGINE_CHILD";
    if std::env::var(CHILD).as_deref() == Ok(test) {
        return true;
    }
    let home = tempfile::tempdir().unwrap();
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test, "--nocapture", "--include-ignored"])
            .env(
                "RUST_MIN_STACK",
                crate::engine_runtime::WORKER_THREAD_STACK_SIZE.to_string(),
            )
            .env(CHILD, test)
            .env("HOME", home.path())
            .env_remove("LUCIDOS_GATEWAY_DATA")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "the child must execute exactly one test: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    false
}

pub fn run_on_engine_runtime<M, F>(name: &'static str, make_test: M)
where
    M: FnOnce() -> F + Send + 'static,
    F: std::future::Future<Output = ()>,
{
    let test = std::thread::Builder::new()
        .stack_size(crate::engine_runtime::WORKER_THREAD_STACK_SIZE)
        .spawn(move || {
            crate::engine_runtime::build_runtime()
                .expect("the engine runtime builds")
                .block_on(async move {
                    if in_engine_child(name).await {
                        make_test().await;
                    }
                })
        })
        .expect("the test thread starts");
    if let Err(panic) = test.join() {
        std::panic::resume_unwind(panic);
    }
}

pub async fn seed_coding_thread(bus: &EventBus, parent: Option<Uuid>) -> Uuid {
    let thread_id = Uuid::new_v4();
    seed_coding_thread_as(bus, thread_id, parent).await;
    thread_id
}

pub async fn seed_coding_thread_as(bus: &EventBus, thread_id: Uuid, parent: Option<Uuid>) {
    emit_cc(bus, thread_id, message_from_parent("work", parent)).await;
    start_cc_session(bus, thread_id, &format!("claude-code/{thread_id}"), None).await;
}

pub async fn seed_idle_coding_thread(bus: &EventBus, parent: Option<Uuid>) -> Uuid {
    let thread_id = seed_coding_thread(bus, parent).await;
    end_coding_turn(bus, thread_id).await;
    thread_id
}

pub async fn end_coding_turn(bus: &EventBus, thread_id: Uuid) {
    emit_cc(
        bus,
        thread_id,
        ThreadEvent::CodingAgentIdled {
            has_changes: false,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: None,
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
            turn_end: None,
        },
    )
    .await;
}

pub async fn seed_hold(bus: &EventBus, thread_id: Uuid) -> Uuid {
    let hold_id = Uuid::new_v4();
    seed_thread_event(
        bus,
        thread_id,
        ThreadEvent::ThreadHoldPlaced {
            reason: "test hold".into(),
            hold_id,
            root_thread_id: thread_id,
        },
    )
    .await;
    hold_id
}

/// INVARIANT: A legacy *successor thread* chain: `old` continued in a new thread, which
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

pub fn event_wait_started(wait_id: Uuid) -> ThreadEvent {
    ThreadEvent::EventWaitStarted {
        background_timing: None,
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

pub async fn seed_device(pool: &PgPool, id: &str, user_agent: Option<&str>, name: Option<&str>) {
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    crate::core::DeviceStore::register(pool, &bus, id, user_agent, None)
        .await
        .unwrap_or_else(|e| panic!("seed device {id}: {e}"));
    if name.is_some() {
        crate::core::DeviceStore::rename(pool, &bus, id, name, None)
            .await
            .unwrap_or_else(|e| panic!("name device {id}: {e}"));
    }
}

#[allow(clippy::too_many_arguments)]
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

#[allow(clippy::too_many_arguments)]
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

pub async fn seed_preference(pool: &PgPool, key: &str, value: &str) -> Result<(), sqlx::Error> {
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    crate::core::PreferenceStore::set(pool, &bus, key, value, None).await
}

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

pub async fn seed_chatgpt_plan_sign_in(pool: &PgPool, access_token: &str) {
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    crate::llm::chatgpt_plan::ChatGptPlanAccountStore::seed_sign_in(pool, &bus, access_token).await;
}

pub async fn seed_preference_for_device(
    pool: &PgPool,
    key: &str,
    value: &str,
    device_id: &str,
) -> Result<(), sqlx::Error> {
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    crate::core::PreferenceStore::set_for_device(pool, &bus, key, value, device_id, None).await
}

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

pub fn offline_event_bus() -> EventBus {
    let (bus, _callback_rx) = EventBus::new(offline_pool());
    bus
}

pub fn offline_pool() -> PgPool {
    PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(50))
        .connect_lazy("postgres://lucidos:lucidos@127.0.0.1:1/offline")
        .expect("lazy pool")
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

pub mod source_scan {
    use std::path::{Path, PathBuf};

    pub fn src_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
    }

    fn source_roots() -> [PathBuf; 3] {
        let root = src_root();
        [
            root.clone(),
            root.join("../../lucidos-engine/src"),
            root.join("../../lucidos-agents-api/src"),
        ]
    }

    pub fn engine_source(rel: &str) -> PathBuf {
        source_roots()
            .into_iter()
            .map(|root| root.join(rel))
            .find(|path| path.exists())
            .unwrap_or_else(|| src_root().join(rel))
    }

    pub fn is_test_path(rel: &str) -> bool {
        rel.split('/').any(|part| {
            let base = part.strip_suffix(".rs").unwrap_or(part);
            base == "tests" || base == "bin" || base.ends_with("_test") || base.ends_with("_tests")
        })
    }

    pub fn read_production_source(path: &Path) -> String {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        if text.lines().any(|line| {
            matches!(
                line.trim(),
                "#![cfg(test)]" | r#"#![cfg(any(test, feature = "test-support"))]"#
            )
        }) {
            return String::new();
        }
        match text.find("\n#[cfg(test)]") {
            Some(idx) => text[..idx].to_string(),
            None => text,
        }
    }

    pub fn production_sources() -> Vec<(String, String)> {
        let mut out = Vec::new();
        let boot = "../../lucidos-engine-boot/src";
        let roots = source_roots()
            .into_iter()
            .map(|root| ("", root))
            .chain([(boot, src_root().join(boot))]);
        for (prefix, source_root) in roots {
            let mut stack = vec![source_root.clone()];
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
                        .strip_prefix(&source_root)
                        .expect("under src")
                        .to_string_lossy()
                        .replace('\\', "/");
                    if is_test_path(&rel) {
                        continue;
                    }
                    let text = read_production_source(&path);
                    if !text.is_empty() {
                        let rel = if prefix.is_empty() {
                            rel
                        } else {
                            format!("{prefix}/{rel}")
                        };
                        out.push((rel, text));
                    }
                }
            }
        }
        let executable = "../../lucidos-engine-bin/src/main.rs";
        out.push((
            executable.to_owned(),
            read_production_source(&src_root().join(executable)),
        ));
        out.sort();
        out
    }
}

pub fn self_signed_pem() -> (Vec<u8>, Vec<u8>) {
    use openssl::asn1::Asn1Time;
    use openssl::hash::MessageDigest;
    use openssl::pkey::PKey;
    use openssl::rsa::Rsa;
    use openssl::x509::extension::SubjectAlternativeName;
    use openssl::x509::{X509NameBuilder, X509};

    let key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
    let mut subject = X509NameBuilder::new().unwrap();
    subject.append_entry_by_text("CN", "localhost").unwrap();
    let subject = subject.build();

    let mut builder = X509::builder().unwrap();
    builder.set_version(2).unwrap();
    builder.set_subject_name(&subject).unwrap();
    builder.set_issuer_name(&subject).unwrap();
    builder.set_pubkey(&key).unwrap();
    builder
        .set_not_before(&Asn1Time::days_from_now(0).unwrap())
        .unwrap();
    builder
        .set_not_after(&Asn1Time::days_from_now(1).unwrap())
        .unwrap();
    let san = SubjectAlternativeName::new()
        .ip("127.0.0.1")
        .dns("localhost")
        .build(&builder.x509v3_context(None, None))
        .unwrap();
    builder.append_extension(san).unwrap();
    builder.sign(&key, MessageDigest::sha256()).unwrap();

    (
        builder.build().to_pem().unwrap(),
        key.private_key_to_pem_pkcs8().unwrap(),
    )
}

pub struct FakeHookAsker {
    verdict: crate::core::event_hooks::HookVerdict,
    asked: std::sync::Mutex<Vec<crate::core::event_hooks::PendingEvent>>,
    hold: Option<std::sync::Arc<tokio::sync::Notify>>,
}

impl FakeHookAsker {
    pub fn new(verdict: crate::core::event_hooks::HookVerdict) -> std::sync::Arc<Self> {
        Self::held(verdict, None)
    }

    pub fn held(
        verdict: crate::core::event_hooks::HookVerdict,
        hold: Option<std::sync::Arc<tokio::sync::Notify>>,
    ) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            verdict,
            asked: std::sync::Mutex::new(Vec::new()),
            hold,
        })
    }

    pub fn asked(&self) -> Vec<crate::core::event_hooks::PendingEvent> {
        self.asked.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl crate::core::event_hooks::EventHookAsker for FakeHookAsker {
    async fn ask(
        &self,
        ask: crate::core::event_hooks::HookAsk<'_>,
    ) -> crate::core::event_hooks::HookVerdict {
        self.asked.lock().unwrap().push(ask.pending.clone());
        if let Some(hold) = &self.hold {
            hold.notified().await;
        }
        self.verdict.clone()
    }
}

pub fn hook_refuses(reason: &str) -> crate::core::event_hooks::HookVerdict {
    crate::core::event_hooks::HookVerdict::Refused(crate::core::event_hooks::Refusal {
        cause: crate::core::event_hooks::RefusalCause::Refused,
        reason: reason.into(),
        detail: None,
    })
}

pub async fn arm_hook(bus: &EventBus, slug: &str, event_type: &str) {
    bus.emit(BusEvent::System(SystemEvent::EventHookArmed {
        slug: slug.into(),
        event_type: event_type.into(),
        content_hash: "abc".into(),
        actor: None,
    }))
    .await
    .unwrap();
}

pub async fn install_refusing_hook(
    bus: &EventBus,
    slug: &str,
    event_type: &str,
    reason: &str,
) -> std::sync::Arc<FakeHookAsker> {
    let asker = FakeHookAsker::new(hook_refuses(reason));
    bus.install_event_hook_asker(asker.clone()).await.unwrap();
    arm_hook(bus, slug, event_type).await;
    asker
}

pub async fn hook_refusals(pool: &PgPool, slug: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE event_type = 'EventHookRefused' \
         AND payload->'data'->>'slug' = $1",
    )
    .bind(slug)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[cfg(test)]
#[path = "test_support_tests.rs"]
mod tests;
