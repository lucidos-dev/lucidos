---
paths:
  - "crates/lucidos-engine/migrations/**"
  - "**/*.sql"
---

# Database Schema Reference

Lucidos uses **event sourcing**: the `events` table is the central source of truth. Projections such as `thread_summaries` are maintained in-tx by the EventBus. `crates/lucidos-engine/migrations/` is the schema authority, and `SystemEvent` (`crates/lucidos-engine/src/engine/event_bus_system_event.rs`) is the event catalog. Per-table and per-event notes live in [`docs/database-schema-notes.md`](../../docs/database-schema-notes.md) and [`docs/system-events.md`](../../docs/system-events.md).

## Conventions

- **Never edit or delete an applied migration.** sqlx refuses to run when a previously applied version is missing from the resolved set. Fix forward with a new migration.
- **A migration cannot edit a secret's contents.** It sees only ciphertext, so that change runs in Rust after the key loads. A new secret column joins `SECRET_COLUMNS` (`core/secret_store/`) with its `<table>_<column>_sealed` CHECK in the same change, and the guard test fails otherwise (ADR 0509). The agent role gets no write on these tables.
- **`summary_version` is trigger-owned.** Never set or bump it by hand (ADR 0329).
- **Derive a count, never increment it.** A paired list and count come from one statement per projection arm (`jsonb_array_length`), and a roll-up is recounted from ground truth (`rebuild_children_counts`). Two writers of one fact disagree eventually.
- **A projection arm converges on replay.** Filter before append, and upsert rather than insert, so the projection rebuild replays to the same rows.
- **Every write announces.** The emit lives in the store's write path, never at the call site, so every surface announces by construction. A deliberately silent writer is an `ExemptWriter` with its reason in `core/announced_surfaces.rs`. "No variant fits" is a reason to add a variant.
- **An observation is not a decision.** A cache refresh, a delivery stamp or a derived index writes silently. A state change a user or agent chose is an event.
- **A table written by one module stays that way.** Where a table's note says "written only by", the raw writers are module-private, so no path skips the projection or the emit.
- **A CHECK or unique index states the invariant**, for example `events_change_applied_once`, `idx_changes_unique_open_branch` and `webhooks_needs_a_verifier`. Prefer one to a comment or a guard in code.
- **Measure ages in SQL, never against the engine's clock** (ADR 0053).

## Key tables

The schema is in `migrations/`. Notes on who writes each table and which invariant a constraint protects are in `docs/database-schema-notes.md` § Tables.

| Table | Purpose |
|---|---|
| `events` | Event store. Per-thread commit order equals `sequence` order (ADR 0360). A global reader takes its mark from `committed_event_horizon` (ADR 0364). |
| `changes` | Coding-agent proposed changes. One open row per `branch_name`. |
| `thread_summaries` | Projection of cached thread metadata. Its `summary_version` orders every summary the client receives. |
| `thread_queue`, `apply_all_batches`, `standing_applies`, `thread_input_chains` | Admission and apply bookkeeping. Live rows only, with `events` as the audit trail. |
| `hardened_branches`, `planned_branches` | The harden marker and the plan marker. |
| `notifications`, `devices`, `push_subscriptions`, `device_presence`, `pinned_apps`, `preferences` | Notification and device state. |
| `credentials`, `oauth_accounts`, `email_accounts`, `vapid_keys`, `backup_recipient`, `claude_gateway_accounts`, `claude_gateway_telemetry_cursor` | Secrets and accounts. Secret columns hold sealed values only. |
| `environment_variables` | User-managed non-secret variables. |
| `mcp_servers`, `models`, `chat_ladder_upgrade_models`, `repositories` | Registries and configuration. |
| `repo_names` | Projection. Keeps the name of every repo ever added. `RepositoryRemoved` deliberately leaves it alone. |
| `app_kinds` | Projection. Keeps the kind of a removed app folder. |
| `summary_tree_nodes`, `summary_tree_scopes` | The summary tree, a derived index. |
| `memory_entries` | Vector memory with pgvector embeddings (384-dim). |
| `webhooks`, `webhook_deliveries` | Webhook config and the delivery nonce ledger. |
| `browser_logins`, `headless_blocked` | Browser session tracking. |

## Querying threads

No `threads` table. Use:
```sql
SELECT DISTINCT aggregate_id FROM events WHERE event_type = 'CodingAgentIdled';
SELECT payload->>'title' FROM events WHERE aggregate_id = '<id>' AND event_type = 'ThreadTitleGenerated' ORDER BY created DESC LIMIT 1;
SELECT thread_id, branch_name, description, status FROM changes WHERE status = 'pending';
```

The timestamp column is `created`, not `created_at`. `aggregate_id` is `text`, so cast when joining: `aggregate_id = c.thread_id::text`. The `thread_id` column is legacy, so prefer `aggregate_id`. `payload` is `jsonb`.

## Key event types

**Persisted means subscribable** (`docs/adr/0113-persisted-system-events-are-subscribable.md`). A persisted `SystemEvent` is a valid `await_event` name and a valid trigger `on:` entry. A transient frame is refused at registration, because it writes no row for either matcher to see. One predicate decides it for both fan-outs: `core::event_subscription::is_subscribable_system_event`, keyed on `SystemEvent::is_persisted()`. So a new variant needs no separate list, only the persistence decision it already makes.

The catalog is `SystemEvent`, and each variant's doc comment states its payload and emit site. Behaviour that the comments do not carry is in `docs/system-events.md`. The rules below bind every event:

- **Events are immutable and named in the past tense.** The wire shape is `SystemEvent` with `#[serde(tag = "type", content = "data")]`. Rename a variant only with a serde alias, because old rows keep their names.
- **A new variant costs five match arms and a doc comment.** It states its aggregate, `aggregate_id`, payload, and whether it is persisted. Record its projection effect in `docs/system-events.md` when the comment cannot hold it.
- **Audit events carry `actor`**, and the actor merges into the payload through `merge_actor`. An engine-driven event (ingress, supervisor, flaky detection) carries none.
- **`Credential*`, reveal and token events never carry a secret**, only the service name and type. An event that states a set carries the result in full, never a delta.
- **`ChangeApplied` is emitted at most once per `change_id`.** The unique index `events_change_applied_once` and the `ApplyClaim` enforce it (ADR 0442).
- **Resolving a permission never resurrects a thread.** The shared projection arm moves status to `running` only from `waiting_for_user_answer`. `UserQuestionAnswered` is the one unconditional exception.
- **`ThreadsDeleted` carries no `thread_id` and no content**, so no later thread sweep can reach it (ADR 0192). `POST /api/v1/threads/delete` is the only production path that removes an `events` row.
- **A transient frame is never an audit record.** Presence, push, progress and frontend refresh signals write no row.
- **An event the engine emits before the HTTP port binds reaches no SSE subscriber.** Pair it with a `NotificationCreated` when the user must see it (`ProxyConfigRejected`).

## Column notes

Column-level notes (`parent_callback_pending`, `live_event_waits`, `compose_epoch`, the webhook *delivery outcome stamps* and *refusal run*, and the rest) are in `docs/database-schema-notes.md` § Thread projections and § Webhooks. Read them before you change a column, a projection arm, or a writer of one.
