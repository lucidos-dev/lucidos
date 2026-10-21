---
paths:
  - "crates/lucidos-engine/**/*.rs"
  - "Cargo.toml"
  - "Cargo.lock"
---

# Rust Conventions

- **Error handling:** `Box<dyn std::error::Error + Send + Sync>`, with no custom error types. Four are blessed, each because a caller branches on it:
  - `LifecycleViolation` (`engine/thread_lifecycle.rs`): the EventBus rejection path fails fast on its typed payload.
  - `llm::ModelNotServed`: an unset default moves to its next model (ADR 0403).
  - `lucidos_engine_socket::ConnectError` and the gateway's `engine_hop::HopError`: a probe tells `Unreachable` from `Foreign` (ADR 0445).
  - `agent_session::start_failure::AgentStartFailure`: a coding-agent turn that could not begin. `agent_exit_of` attaches `agent_exit` to the `ResponseFailed` (ADR 0487).
  - `api::error::ApiError` is not an error type in this sense. It is the `/api/v1` response shape (`status` plus `{"error": msg}`), and handlers return `Result<_, ApiError>`. Internal code keeps the boxed trait object. `ApiError::with_reason` adds a `reason` slug for a refusal a client must tell apart.
  - A new custom type needs the same structural reason. "I want a typed error for this layer" is not one. If the caller only formats the error into a string, use the boxed trait object.
- **Shared state:** `Arc<Mutex<T>>` for exclusive access, `Arc<RwLock<T>>` for read-heavy.
- **Logging:** use `log!("[Module] message", args)` from `crate::lib`, never raw `println!`/`eprintln!`. Raw output is reserved for the `log!` definition itself and for user-facing CLI output that tooling parses (`--version`, `--help`). A new exception needs the same justification, documented at the call site.
- **Commit messages:** conventional commits (`feat:`, `fix:`, `docs:`, `refactor:`, `chore:`).
- **Path validation:** check user-provided paths for `..`, a leading `/` and a leading `\`.
- **Serialization:** `#[derive(Serialize, Deserialize)]` on all API and event types, `#[derive(sqlx::FromRow)]` on DB types.
- **Tests:** `#[cfg(test)]` modules sit beside their source. Inline `mod tests` is the default. A module where tests dominate lifts them into a sibling file: `#[cfg(test)] #[path = "foo_tests.rs"] mod tests;`, or a `foo_tests/` directory with one file per `mod`. Examples: `event_bus_tests.rs`, `context_tests/`.
- **`.ok()` / `let _ =` only for truly unrecoverable cleanup** (a temp dir). Never for an operation whose failure the user should know about.
- **A probe that could not run is UNKNOWN, never a "no".** `git_cmd` returns `Err` on a spawn failure AND on its 30s timeout, and a saturated host really does take tens of seconds. So `git_cmd(...).map(|o| o.status.success()).unwrap_or(false)` is a coin flip that lands on "gone" when the machine is busiest.
  - Ask yes/no questions through **`git_answer` / `git_answer_with`** (exit status is the answer) or **`git_answer_when_ok`** (a non-zero exit is a failure, as with `git status`). Collapse the `GitAnswer` with **`or_unknown(true|false)`**, which forces you to name the fallback.
  - **Pick the side by asking which direction destroys something.** An unknown never authorizes:
    - deleting a worktree or branch, or discarding a tree,
    - force-checking-out over a working tree,
    - moving a ref with no old-value guard,
    - resolving a change as an already-applied no-op.

    Prefer the direction that keeps the user's work and fails loudly later.
  - The same holds for any string a git call yields: `unwrap_or_default()` turns an unresolved sha into `""`, and two empty shas compare equal.
  - Every current site and its default: `docs/plans/2026-08-03-unknown-git-state-must-not-delete-worktrees.md`.
- **Failure-path cleanup deletes only what the failed attempt created.** A destructive op (`git worktree remove --force`, `git branch -D`, `remove_dir_all`) on an error path never runs unconditionally. On a resume path the target is pre-existing state holding the user's commits. Track what THIS attempt created (`SpawnWorktreeContext::{worktree_created, branch_created}`, `git_ops::cleanup_failed_spawn`) and log every outcome. For a "start fresh" retry, use `remove_worktree_and_optionally_delete_branch`, which keeps a branch with unique commits. When you fix one instance, grep for its siblings.
- **A session teardown never reclaims a worktree (ADR 0035).** The background `WorktreeCleanup` worker is the only owner of reclaiming disk. Two removals remain on the session path, because each deletes something the attempt owns: an explicit user **Discard** (`discarded_worktree_removal`) and the Tier-3 **temp** merge tree (`conflict_abort_deletes_temp_state`). A third fails `the_completion_path_removes_only_the_two_worktrees_it_is_allowed_to`.
- **Never slice strings by byte index.** `&s[..n]` panics on multi-byte chars. Use `s.floor_char_boundary(n)`.
- **API returns raw markdown, frontend converts to HTML.** No `markdown_to_html` on the backend. Error responses use the `[ERROR]` prefix.
- **A state change ANNOUNCES by default.** A write to a table that `core/announced_surfaces.rs` classes `Announced` emits. "There is no variant yet" is a reason to ADD one: a new `SystemEvent` is five match arms plus a doc comment on the variant (`docs/system-events.md` for behaviour the comment cannot hold). `ExemptWriter` covers three shapes only: downstream of an emit, a cascade the parent event covers, or an observation rather than a decision. Its required `why` records that judgment.
- **EVERY model call records what it cost (ADR 0242).** Only the *model call service* (`engine::model_call`) calls a model: each billable provider method takes a `CallToken` that only `llm::metered` mints. Call through `AuxCapture::chat` / `judge` / `generate` / `search` under a `ContextPurpose` of its own, or `turn_chat` for an agent's turn. Spend reported outside a provider call goes through `AuxCapture::record_usage`.
- **ALL events go through EventBus.** `EventBus.emit()` is the sole entry point for persistence. Thread events use `BusEvent::Thread { .. }`, system events `BusEvent::System(SystemEvent::...)`, and domain events (from `emit_event`) `SystemEvent::DomainEvent { .. }`, persisted with the inner event_type. Never write to the events table directly.
- **Response termination uses the typed helpers.** Never construct `ThreadEvent::ResponseCanceled` or `ResponseAborted` directly. Call `thread_events::emit_response_canceled` (user-driven, takes a `CancelCause`) or `emit_response_aborted` (system-driven, takes an `AbortCause`). Two exceptions stay direct, with a comment: sites that need the emit `Err` for per-thread logging (chat/recovery sweep, the `mod.rs` shutdown sweep, `settle_stuck_running_thread`), and `runtime_helpers::make_terminal_event`, the factory the helpers use.
- **A mutating endpoint identifies its caller BEFORE it mutates.** Resolve at the top of the handler with `api::actor::require_user_actor(&headers, &pool, None).await?`, then emit with `emit_or_log(BusEvent::System(SystemEvent::X { …, actor: Some(actor) }), "[Module] EventName")`. A handler returning a bare `Response` uses `require_user_actor_response`.
  - **Never resolve the actor at emit time.** The emit runs after the write, so a refusal there is a mutation already recorded under nobody. `EventBus::emit_user_system` did that and was deleted.
  - **A caller presenting no credential is refused, not stamped as the user** (ADR 0169). `user_actor` keeps the `Option` for a read path. Cross-workspace handlers use `build_message_origin` with a `CallerOrigin`. Per-device handlers pass the body's device id as the override. Engine-internal emits (state machine, scheduler tick, recovery) pass `None` and call `emit_or_log`.
  - **`api::mutating_gate` already refuses every mutating method under `/api/v1`.** A new handler resolves with `user_actor` and treats the `Option` as `Some`. A route nobody can identify itself on goes in its exemption list, with a reason.
  - **A device actor carries the id only** (`MessageOrigin::Device { device_id }`). A display resolves the name through `DeviceStore::friendly_name`. A new `ThreadEvent` variant uses `EventMeta::with_actor(actor)`. A `SystemEvent` variant adds `actor: Option<MessageOrigin>` as a `#[serde(default, skip_serializing_if = "Option::is_none")]` field.
- **What the AGENT wrote names which agent.** An agent's own output stamps `EventMeta::authored_by(AgentParticipant::…)` (ADR 0150). Stamp the AUTHORED event only, never the turn's shared request meta, or a user's Stop click is attributed to the agent.
- **Dependencies: the committed lockfile is authoritative, and builds are `--locked` (ADR 0020).** Keep caret ranges in `Cargo.toml` (`serde = "1"`), never `=` pins. Commit `Cargo.toml` and `Cargo.lock` together. Change a version only through a deliberate `cargo update`.
- **Schema changes are migrations.** Create one with `./scripts/new-migration.sh <description>`, never a hand-picked timestamp (parallel branches collide and crash startup on `_sqlx_migrations_pkey`). The file is `YYYYMMDDHHMMSS_description.sql` in `crates/lucidos-engine/migrations/`. Never put ALTER TABLE in `init_schema`.

## Database

Lucidos uses **event sourcing**: the `events` table is the source of truth, with projections (`thread_summaries`, `notifications`) maintained by EventBus. The schema reference lives in `.claude/rules/db.md`.

- **"The last N seconds" over a DB timestamp is resolved in SQL, never from `Utc::now()` (ADR 0053).** Pass a duration (`window_secs: i64`) and write `created >= now() - make_interval(secs => $n)`. An age is `EXTRACT(EPOCH FROM now() - created)::bigint`.

## Timezone Handling

UTC in the database, user timezone in the UI. Cron has 6 fields: `second minute hour day-of-month month day-of-week` (`0 0 8 * * *` is 8am daily). It is stored with the user timezone for DST, in IANA format (`Europe/Oslo`). Convert with `utc_time.with_timezone(&user_tz)`.

## HTTP Usage Rule

Only conversational data goes through the HTTP API. DB access, vector search, embeddings and file ops stay in-process.

## Intra-host scheme: never hardcode `http`/`https`

An HTTP call between co-located Lucidos processes resolves its scheme through **`net_config::tls_scheme` / `tls_scheme_from` / `peer_scheme_order`** (`SCHEME_HTTP` / `SCHEME_HTTPS`). That covers engine to gateway control API and any `127.0.0.1` hop. The decision lives in `net_config` only. Never re-derive it from `LUCIDOS_TLS_*` inline.

- A process serves TLS iff both `LUCIDOS_TLS_CERT` and `LUCIDOS_TLS_KEY` point at non-empty paths. Dev keeps the certs (`https`). The packaged gateway strips them (`http`), per ADR 0014.
- For a peer whose scheme might differ, use `peer_scheme_order()`: the resolved scheme first, the other second.
- A self-signed dev cert on a loopback hop is accepted with `danger_accept_invalid_certs(true)` + `no_proxy()`.
- **The gateway's hop to an engine has no scheme.** It uses the *engine socket* through `engine_hop` (ADR 0445).
- **One exception: the Vertex relay** (`runtime/vertex_relay.rs`) never serves TLS, so its `http://` is fixed (ADR 0260).

## API URL Conventions

**Every HTTP route lives under `/api/v1/`.** The engine mounts one router there (`create_router` in `crates/lucidos-engine/src/api/mod.rs`). Define a new route relative to the mount (`/threads/list`). The nest stamps the prefix once.

- Frontend callers build URLs from the single `API` const in `crates/lucidos-app/src/api/client.ts`.
- SDK callers pass suffix-only paths (`request('/threads/list')`). `_fetch.ts` stamps the prefix. For a full URL (`EventSource`, `<script src>`, hrefs) use `apiUrl('/events')`.
- Engine-internal callers (`workspace_client.rs`) hit the target's `/api/v1/...` directly.
- Docs and comments name routes by their full path, `/api/v1/` included.
- Prefer query params: `/apps?commit=abc123`.

## Apps: Event APIs

When you change the event system, update all 5 surfaces:
1. LLM tool definitions: `llm/tools/`
2. LLM tool handlers: `engine/tools/mod.rs`
3. App UI JS API: `api/app_ui.rs`
4. HTTP API handlers: `api/history.rs`
5. System prompt: `engine/chat/process/system_prompt.rs`

`emit_event(event_type, payload)` takes a PascalCase past-tense type, and the payload must include `summary`. `query_events(event_id?, event_type?, thread_id?, since?, until?, limit?, byte_limit?)` returns newest first. `limit` defaults to 50 (max 200). `byte_limit` defaults to 128 KB (max 512 KB). `event_id` resolves ONE event by primary key (the `evt-<32 hex>` address) and errors when nothing matches. `thread_id` takes a uuid, or `current` / `this` on the LLM tool only, since HTTP and SDK have no ambient caller.

**A thread id an agent or app writes is settled by `api::resolve_thread_id_arg`, never stored raw.** A slot resolves the alias or refuses the value. Match on the VALUE, not `as_str()`. `notifications::resolve_thread_tap_id` settles a stored tap, and `api::resolve_thread_id_in_nav_payload` settles a `NavigationRequested` payload. An HTTP or SDK caller passes `None`, which refuses the alias. Reading stays permissive, so rows written before the guard stay repairable.
