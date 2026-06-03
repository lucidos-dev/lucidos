---
globs:
  - "crates/lucidos-engine/**/*.rs"
  - "Cargo.toml"
  - "Cargo.lock"
---

# Rust Conventions

- **Error handling:** `Box<dyn std::error::Error + Send + Sync>` — no custom error types. **One blessed exception**: `LifecycleViolation` in `engine/thread_lifecycle.rs` carries structured fields (`event_type`, `thread_type`, `current_section`, `reason`) so the EventBus rejection path can fail-fast on a typed payload instead of parsing a string. New custom error types need the same kind of structural justification — generic "I want a typed error for this layer" is not enough; if the caller only ever formats the error into a string, use the boxed trait object.
- **Shared state:** `Arc<Mutex<T>>` for exclusive access, `Arc<RwLock<T>>` for read-heavy
- **Logging:** Always use `log!(...)` from `crate::lib` instead of raw `println!`/`eprintln!`. Format: `log!("[Module] message", args)` with `[Module]` prefix. Raw `println!`/`eprintln!` are reserved for: (1) inside the `log!` macro definition itself, and (2) user-facing CLI output that external tooling parses — `--version`/`-V` and `--help` are the canonical cases, where `log!`'s timestamp/pid/label prefix would break parsers reading stdout. Carve out a new exception only with the same justification (machine-readable stdout that callers depend on), and document the rationale at the call site.
- **Commit messages:** Conventional commits — `feat:`, `fix:`, `docs:`, `refactor:`, `chore:`
- **Path validation:** Always check for `..`, leading `/`, leading `\` before accepting user-provided paths
- **Serialization:** `#[derive(Serialize, Deserialize)]` on all API/event types, `#[derive(sqlx::FromRow)]` for DB types
- **Tests:** `#[cfg(test)]` modules colocated with source. Inline `mod tests { ... }` is the default; for large modules where tests dominate the file, lift them into a sibling file via `#[cfg(test)] #[path = "foo_tests.rs"] mod tests;` (or a sibling directory `foo_tests/` with one file per `mod` when there are multiple top-level test modules). Examples in tree: event_bus_tests.rs, context_tests/, thread_lifecycle_tests/.
- **`.ok()` / `let _ =` only for truly unrecoverable cleanup** (e.g. removing temp dirs). Never for operations whose failure the user should know about.
- **Never slice strings by byte index.** `&s[..n]` panics on multi-byte chars. Always use `s.floor_char_boundary(n)`.
- **API returns raw markdown, frontend converts to HTML.** No `markdown_to_html` on backend. Error responses use `[ERROR]` prefix.
- **ALL events go through EventBus.** `EventBus.emit()` is the sole entry point for event persistence. `EventStore::append()` and `append_thread_event()` have been deleted. Thread events use `BusEvent::Thread { .. }`, system events use `BusEvent::System(SystemEvent::...)`, and domain events (from `emit_event`) use `SystemEvent::DomainEvent { .. }` (persisted with the inner event_type, not "DomainEvent"). Never bypass EventBus to write directly to the events table.
- **Response termination uses the typed helpers.** Never construct `ThreadEvent::ResponseCanceled { .. }` or `ThreadEvent::ResponseAborted { .. }` directly — call `thread_events::emit_response_canceled` (cancel = user-driven, takes a `CancelCause`) or `thread_events::emit_response_aborted` (abort = system-driven, takes an `AbortCause`). The helpers force every site to declare *why* the response ended via the typed enum, and route the construction through one place so the cancel-vs-abort split stays honest. Exceptions stay direct and document why with a comment: (1) sites that need to observe the emit `Err` for per-thread logging — chat/recovery sweep, mod.rs shutdown sweep, settle_stuck_running_thread; (2) `runtime_helpers::make_terminal_event` — the lower-level factory the helpers themselves depend on.
- **Mutating endpoints stamp the actor.** Use `state.engine.event_bus.emit_user_system(&headers, &state.pool, "[Module] EventName", |actor| SystemEvent::X { …, actor }).await` from any axum handler that mutates state and emits a `SystemEvent`. The helper resolves the actor via `api::actor::user_actor_resolved`, wraps in `BusEvent::System`, and emits through `emit_or_log` so failures land in `[EventBus] [Module] EventName emit failed: …`. Use the lower-level `user_actor_resolved` + `emit_or_log` directly only for the cases the helper deliberately excludes: per-device handlers (body-supplied device id), cross-workspace handlers (use `build_message_origin` with a `CallerOrigin`), `BusEvent::Thread` emits, and handlers that need the `EmitResult` back. Engine-internal emits (state-machine side effects, scheduler tick, recovery) pass `None` and call `emit_or_log` directly. The lower-level `user_actor(&headers, device_id, device_label)` exists for callers that already have the label in hand. For new `ThreadEvent` variants prefer `EventMeta::with_actor(actor)`. The four change events carry per-variant `actor` (predates EventMeta path); leave as-is. For `SystemEvent` add `actor: Option<MessageOrigin>` as a `#[serde(default, skip_serializing_if = "Option::is_none")]` field on the variant. The actor flows into the event payload as a stable `actor` field — frontend reads it without translation.
- **Database schema changes must be migrations.** Always create with `./scripts/new-migration.sh <description>` — it stamps the file with the real wall-clock second and bumps if the slot is taken. Never hand-pick the timestamp (placeholders like `120000` collide across parallel CC branches and crash the engine on startup with a `_sqlx_migrations_pkey` duplicate). Format: `YYYYMMDDHHMMSS_description.sql` in `crates/lucidos-engine/migrations/`. `build.rs` panics if two files share a version prefix. Never put ALTER TABLE in `init_schema`.

## Database

Lucidos uses **event sourcing** — the `events` table is the source of truth, with cached projections (`thread_summaries`, `notifications`) maintained by EventBus. Schema reference (tables, event types, column notes, query patterns) lives in `.claude/rules/db.md` — auto-loaded when editing `*.sql` or `migrations/**`.

## Timezone Handling

UTC in database, user timezone in UI. Cron: 6 fields — `second minute hour day-of-month month day-of-week` (e.g., `0 0 8 * * *` = 8am daily). Stored with user timezone for DST. IANA format (e.g., `Europe/Oslo`). Convert: `utc_time.with_timezone(&user_tz)`.

## HTTP Usage Rule

Only conversational data goes through HTTP API. DB access, vector search, embeddings, file ops stay in-process.

## API URL Conventions

**Every HTTP route lives under `/api/v1/`.** No exceptions — no `/api/foo` shortcuts, no unversioned routes. The engine mounts a single router at `/api/v1` (see `create_router` in `crates/lucidos-engine/src/api/mod.rs`); a request to `/api/foo` returns 404. When you add a new endpoint, define it relative to that mount (`/health`, `/threads/list`, `/foo/bar`) — the `/api/v1/` prefix is stamped on once by the nest, not repeated per `.route()`.

Frontend callers in `crates/lucidos-app/src/api/client.ts` build URLs from a single `const API = ${API_BASE}/api/v1` so the version cannot drift per-function. SDK callers (`packages/lucidos-sdk/src/`) pass *suffix-only* paths (`request('/threads/list')`); the `/api/v1` prefix is stamped inside `_fetch.ts` (`API_V1` constant + `apiUrl()` helper). When you need a full URL for `EventSource`, `<script src>`, or anchor hrefs, use `apiUrl('/events')` rather than concatenating manually. Engine-internal callers (e.g. `crates/lucidos-engine/src/engine/http/workspace_client.rs` reaching across workspaces) hit the target's `/api/v1/...` directly — no engine code should ever emit a `/api/` URL without the version.

Docs (`system-knowhow/**`, `docs/**`, comments in code) reference routes by their full path including `/api/v1/`. A doc that says "POST /api/foo" is wrong — the route either is `/api/v1/foo` (fix the doc) or doesn't exist.

Prefer query params: `/apps?commit=abc123`, `/app?id=habit-tracker`.

## Apps — Event APIs

When changing event system, update all 5 surfaces:
1. LLM tool definitions — `llm/tools.rs`
2. LLM tool handlers — `engine/tools/mod.rs`
3. App UI JS API — `api/app_ui.rs`
4. HTTP API handlers — `api/history.rs`
5. System prompt — `engine/prompt.rs`

LLM event tools: `emit_event(event_type, payload)` — PascalCase past tense, payload must include `summary`. `query_events(event_type?, since?, until?, limit?)` — newest first, default 100, max 1000.
