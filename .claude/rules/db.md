---
globs:
  - "crates/lucidos-engine/migrations/**"
  - "**/*.sql"
---

# Database Schema Reference

Lucidos uses **event sourcing** — the `events` table is the central source of truth. Thread metadata cached in `thread_summaries` (projection maintained by EventBus).

## Key tables

| Table | Purpose |
|---|---|
| `events` | Event store. Columns: `id` (uuid), `event_type` (text), `payload` (jsonb), `created` (timestamptz), `thread_id` (uuid), `sequence` (bigserial), `aggregate` (text), `aggregate_id` (text). |
| `changes` | CC proposed changes. Columns: `id`, `request_id`, `branch_name`, `repo_root`, `description`, `file_count`, `files` (text[]), `requires_restart`, `status` (`pending`/`applied`/`discarded`), `created_at`, `resolved_at`, `thread_id`. |
| `thread_summaries` | Projection — cached thread metadata (title, source, last_activity, message_count, is_saved, has_response). Maintained by EventBus. |
| `notifications` | Projection — from `NotificationCreated` events. |
| `preferences` | Key-value store with optional `device_id` scoping. |
| `memory_entries` | Vector memory with pgvector embeddings (384-dim). |
| `credentials` | Service credentials. |
| `devices` / `push_subscriptions` | Push notification targets. |
| `mcp_servers` | MCP server configs. |
| `oauth_accounts` / `email_accounts` | External account configs. |
| `pinned_apps` | Pinned app UIs per device. |
| `browser_logins` / `headless_blocked` | Browser session tracking. |

## Querying threads

No `threads` table. Use:
```sql
SELECT DISTINCT aggregate_id FROM events WHERE event_type = 'CodingAgentIdled';
SELECT payload->>'title' FROM events WHERE aggregate_id = '<id>' AND event_type = 'ThreadTitleGenerated' ORDER BY created DESC LIMIT 1;
SELECT thread_id, branch_name, description, status FROM changes WHERE status = 'pending';
```

## Key event types

- **Thread lifecycle:** `SessionStarted`, `SessionEnded`, `ThreadTitleGenerated`, `ThreadSaved`/`ThreadUnsaved`/`ThreadArchived`
- **Chat / triggers (Lucidos Agent):** `MessageReceived`, `ResponseGenerated`, `ResponseCanceled`, `ResponseAborted`, `TextStreamed`, `Thinking`. These exist on chat AND CC threads — `MessageReceived` is the user typing, regardless of which agent answers. To filter chat-only, join `thread_summaries` and gate on `source = 'chat' AND is_cc = false` (or `source IN ('chat','trigger')` to include triggers).
- **Coding Agent (CC):** `CodingAgentUserMessageSent` (user-typed), `CodingAgentPromptSent` (engine-synthesized: hardening, conflict recovery), `CodingAgentTextStreamed`, `CodingAgentToolCalled`, `CodingAgentToolResult`, `CodingAgentIdled`, `CodingAgentSettingsChanged`. Old `ClaudeCode*` names are serde aliases — write new code against `CodingAgent*`.
- **Changes:** `ChangeProposed`, `ChangeApplied`, `ChangeDiscarded`
- **Tools:** `ToolCalled`, `ToolResult`, `HttpRequestSent`, `HttpResponseReceived`, `BackgroundBashStarted`, `BackgroundBashCompleted` (the latter two form the durable record for the `run_bash_background` / `bash_output` / `bash_kill` chat tools — `bash_output` falls back to `BackgroundBashCompleted` after the in-memory registry evicts the task on completion)
- **System:** `NotificationCreated`, `PreferencesChanged` (event-sourced, with projections)
- **Scheduling:** `ScheduledTaskCreated`, `ScheduledTaskStarted`, `ScheduledTaskCompleted`, `ScheduledTaskDeleted`
- **Artifacts:** `ArtifactCreated`, `ArtifactUpdated`, `ArtifactDeleted`, `ArtifactImported`, `RepositoryImported`
- **Preferences:** `LanguageSet`, `TimezoneSet`
- **Other:** `SessionResumed`, `ChangeDiscarded`, `ScheduledTriggerCompleted`

## Column notes

Timestamp is `created` (not `created_at`). `aggregate_id` is `text` (not uuid) — cast when joining: `aggregate_id = c.thread_id::text`. `thread_id` column is legacy; prefer `aggregate_id`. `payload` is `jsonb`.
