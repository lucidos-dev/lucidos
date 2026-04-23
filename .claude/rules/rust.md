---
globs:
  - "crates/cognos-engine/**/*.rs"
  - "Cargo.toml"
  - "Cargo.lock"
  - "crates/cognos-engine/migrations/**"
---

# Rust Conventions

- **Error handling:** `Box<dyn std::error::Error + Send + Sync>` — no custom error types
- **Shared state:** `Arc<Mutex<T>>` for exclusive access, `Arc<RwLock<T>>` for read-heavy
- **Logging:** Always use `log!(...)` from `crate::lib` instead of raw `println!`/`eprintln!`. Format: `log!("[Module] message", args)` with `[Module]` prefix. Raw `println!`/`eprintln!` must only appear inside the `log!` macro definition itself.
- **Commit messages:** Conventional commits — `feat:`, `fix:`, `docs:`, `refactor:`, `chore:`
- **Path validation:** Always check for `..`, leading `/`, leading `\` before accepting user-provided paths
- **Serialization:** `#[derive(Serialize, Deserialize)]` on all API/event types, `#[derive(sqlx::FromRow)]` for DB types
- **Tests:** `#[cfg(test)]` modules colocated with source. Inline `mod tests { ... }` is the default; for large modules where tests dominate the file, lift them into a sibling file via `#[cfg(test)] #[path = "foo_tests.rs"] mod tests;` (or a sibling directory `foo_tests/` with one file per `mod` when there are multiple top-level test modules). Examples in tree: event_bus_tests.rs, context_tests/, thread_lifecycle_tests/.
- **`.ok()` / `let _ =` only for truly unrecoverable cleanup** (e.g. removing temp dirs). Never for operations whose failure the user should know about.
- **Never slice strings by byte index.** `&s[..n]` panics on multi-byte chars. Always use `s.floor_char_boundary(n)`.
- **API returns raw markdown, frontend converts to HTML.** No `markdown_to_html` on backend. Error responses use `[ERROR]` prefix.
- **ALL events go through EventBus.** `EventBus.emit()` is the sole entry point for event persistence. `EventStore::append()` and `append_thread_event()` have been deleted. Thread events use `BusEvent::Thread { .. }`, system events use `BusEvent::System(SystemEvent::...)`, and domain events (from `emit_event`) use `SystemEvent::DomainEvent { .. }` (persisted with the inner event_type, not "DomainEvent"). Never bypass EventBus to write directly to the events table.
- **Mutating endpoints stamp the actor.** Any HTTP handler that mutates state (POST/PUT/PATCH/DELETE) must build an actor via `api::actor::user_actor(&headers, ..)` and pass it to the resulting event. For new `ThreadEvent` variants prefer `EventMeta::with_actor(actor)`. The four change events carry per-variant `actor` (predates EventMeta path); leave as-is. For `SystemEvent` add `actor: Option<MessageOrigin>` as a `#[serde(default, skip_serializing_if = "Option::is_none")]` field on the variant. Engine-internal emits (state-machine side effects, scheduler tick, recovery) pass `None`. The actor flows into the event payload as a stable `actor` field — frontend reads it without translation.
- **Database schema changes must be migrations.** Always create with `./scripts/new-migration.sh <description>` — it stamps the file with the real wall-clock second and bumps if the slot is taken. Never hand-pick the timestamp (placeholders like `120000` collide across parallel CC branches and crash the engine on startup with a `_sqlx_migrations_pkey` duplicate). Format: `YYYYMMDDHHMMSS_description.sql` in `crates/cognos-engine/migrations/`. `build.rs` panics if two files share a version prefix. Never put ALTER TABLE in `init_schema`.

## Database Design

CognOS uses **event sourcing** — the `events` table is the central source of truth. Thread metadata cached in `thread_summaries` (projection maintained by EventBus).

### Key tables

| Table | Purpose |
|---|---|
| `events` | Event store. Columns: `id` (uuid), `event_type` (text), `payload` (jsonb), `created` (timestamptz), `thread_id` (uuid), `sequence` (bigserial), `aggregate` (text), `aggregate_id` (text). |
| `changes` | CC proposed changes. Columns: `id`, `request_id`, `branch_name`, `repo_root`, `description`, `file_count`, `files` (text[]), `requires_restart`, `status` (`pending`/`applied`/`discarded`), `created_at`, `resolved_at`, `thread_id`. |
| `thread_summaries` | Projection — cached thread metadata (title, source, last_activity, message_count, is_pinned, has_response). Maintained by EventBus. |
| `notifications` | Projection — from `NotificationCreated` events. |
| `preferences` | Key-value store with optional `device_id` scoping. |
| `memory_entries` | Vector memory with pgvector embeddings (384-dim). |
| `credentials` | Service credentials. |
| `devices` / `push_subscriptions` | Push notification targets. |
| `mcp_servers` | MCP server configs. |
| `saved_contexts` | Saved LLM context snapshots. |
| `oauth_accounts` / `email_accounts` | External account configs. |
| `pinned_apps` | Pinned app UIs per device. |
| `browser_logins` / `headless_blocked` | Browser session tracking. |

### Querying threads

No `threads` table. Use:
```sql
SELECT DISTINCT aggregate_id FROM events WHERE event_type = 'ClaudeCodeIdled';
SELECT payload->>'title' FROM events WHERE aggregate_id = '<id>' AND event_type = 'ThreadTitleGenerated' ORDER BY created DESC LIMIT 1;
SELECT thread_id, branch_name, description, status FROM changes WHERE status = 'pending';
```

### Key event types

- **Thread lifecycle:** `SessionStarted`, `SessionEnded`, `ThreadTitleGenerated`, `ThreadPinned`/`ThreadUnpinned`
- **Chat:** `MessageReceived`, `ResponseGenerated`, `ResponseCanceled`, `ResponseAborted`, `TextStreamed`, `Thinking`
- **Claude Code:** `ClaudeCodeUserMessageSent`, `ClaudeCodeTextStreamed`, `ClaudeCodeToolCalled`, `ClaudeCodeToolResult`, `ClaudeCodeIdled`
- **Changes:** `ChangeProposed`, `ChangeApplied`, `ChangeDiscarded`
- **Tools:** `ToolCalled`, `ToolResult`, `HttpRequestSent`, `HttpResponseReceived`
- **System:** `NotificationCreated`, `PreferencesChanged` (event-sourced, with projections)
- **Scheduling:** `ScheduledTaskCreated`, `ScheduledTaskStarted`, `ScheduledTaskCompleted`, `ScheduledTaskDeleted`
- **Artifacts:** `ArtifactCreated`, `ArtifactUpdated`, `ArtifactDeleted`, `ArtifactImported`, `RepositoryImported`
- **Preferences:** `LanguageSet`, `TimezoneSet`
- **Other:** `SessionResumed`, `ChangeDiscarded`, `ScheduledTriggerCompleted`

### Column notes

- Timestamp is `created` (not `created_at`). `aggregate_id` is `text` (not uuid) — cast when joining: `aggregate_id = c.thread_id::text`. `thread_id` column is legacy; prefer `aggregate_id`. `payload` is `jsonb`.

## Timezone Handling

UTC in database, user timezone in UI. Cron: 6 fields — `second minute hour day-of-month month day-of-week` (e.g., `0 0 8 * * *` = 8am daily). Stored with user timezone for DST. IANA format (e.g., `Europe/Oslo`). Convert: `utc_time.with_timezone(&user_tz)`.

## HTTP Usage Rule

Only conversational data goes through HTTP API. DB access, vector search, embeddings, file ops stay in-process.

## API URL Conventions

Prefer query params: `/apps?commit=abc123`, `/app?id=habit-tracker`.

## Apps — Event APIs

When changing event system, update all 5 surfaces:
1. LLM tool definitions — `llm/tools.rs`
2. LLM tool handlers — `engine/tools/mod.rs`
3. App UI JS API — `api/app_ui.rs`
4. HTTP API handlers — `api/history.rs`
5. System prompt — `engine/prompt.rs`

LLM event tools: `emit_event(event_type, payload)` — PascalCase past tense, payload must include `summary`. `query_events(event_type?, since?, until?, limit?)` — newest first, default 100, max 1000.
