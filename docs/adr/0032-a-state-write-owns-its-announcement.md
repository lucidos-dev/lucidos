# 0032: A state write owns its announcement

- **Status**: Accepted
- **Date**: 2026-08-01

## Context

The `manage_repositories` agent tool wrote a `repositories` row and emitted
nothing. The durable `repo_names` projection never got an entry and the SSE arm
that reloads every client's repository list never fired, so a repo the user
asked for in chat stayed invisible until they reloaded the page, and removing it
left the thread filter showing a raw UUID.

Nothing about that tool was unusual. It called the same `RepositoryStore` the
HTTP handler calls; it just did not also emit, because emitting was the caller's
job and this caller was written later. Every store in the engine was shaped that
way, so each was one forgetful call site away from the same bug.

A sweep found the shape everywhere and three more instances of it already live:

| Where | What was dropped |
|---|---|
| `core/oauth.rs::run_oauth_flow` | Connecting an OAuth account emitted nothing, while `OAuthAccountDeleted` existed and the frontend reloaded on it. Disconnect refreshed every client; connect refreshed none. |
| `engine/tools/image.rs` | A generated or saved image landed in `data/artifacts/` with no emit at all, so it reached no artifact list and `memory_consumer` never indexed it. |
| `api/data_api.rs` | A CLI/SDK write emitted only the `DataFile*` audit event, never the paired `Artifact*`, so CLI-written artifacts were never indexed. The frontend carried a workaround arm whose comment described the gap. |
| `mcp_servers` | No `SystemEvent` existed at all, so registering an MCP server from chat changed the agent's own tool surface with no trace. |

Two more writers (`TaskScheduler::set_backup_schedule`, `api/backup.rs::set_retention`)
bypassed the preference chokepoint and hand-rolled the emit at their single call
site, which is the same failure one step before it happens.

The duplication was visible in the codebase before anyone named it.
`SystemEvent::artifact_change(file_exists, …)` exists only because five callers
each had to make the same Created-vs-Updated decision, capturing
`artifact_exists` BEFORE their own write. One of them did not.

## Decision

**The write path owns the announcement.** A module that mutates an announced
surface emits from inside its write path; the raw writers are private, so no
caller anywhere in the crate can change the state without the emit being
attempted.

**Every surface is classified in one place.**
`crates/lucidos-engine/src/core/announced_surfaces.rs` names every Postgres
table and every `data/` writer as one of:

- `Announced { events, exempt }`: the owning module emits. Reachable writers
  must emit; a deliberate exception needs a named reason on the entry.
- `Projection { of }`: the table materializes an already-announced event
  stream, so its writes are the *result* of an announcement.
- `Silent { reason }`: engine-internal state nothing observes.

**Silence is asked for by name, never by omission.** `PreferenceStore::set_silent`
rejects any key absent from `SILENT_PREF_KEYS`.
`ArtifactManager::write_and_commit` takes a `WriteAnnouncement`, whose only
non-emitting arm (`SupersededBy`) names the richer event the caller emits
instead. Neither has an option that means "nothing", because forgetting is the
failure being designed against.

**Deterministic tests, not review.** Five source-scan tests plus one DB-backed
one enforce the registry: schema completeness, write ownership, an emit on every
reachable writer of an announced surface, real event names, and live exemptions.

## Consequences

- The failure that started this is now a red test rather than a bug report. A
  new table has to be classified before it can ship; a raw `INSERT INTO` outside
  its owning module fails.
- Signatures got wider. Store mutators take `&EventBus` and
  `Option<MessageOrigin>`, so HTTP handlers resolve the actor up front instead
  of going through `emit_user_system` afterwards. Test fixtures seed through
  `test_support::seed_*` helpers rather than each building a bus.
- **A data-API write now also emits the entity event.** Deliberate, with two
  intended consequences: CLI/SDK-written artifacts start being indexed into
  memory, and an `on_event: ArtifactCreated` trigger starts seeing them. The
  `DataFile*` audit family stays as the API-origin record.
- **The guarantee is reachability, not atomicity.** The row or file commits in
  its own transaction and the emit follows through `emit_or_log`, the engine's
  fire-and-forget contract. A transient emit failure costs one live refresh and
  is logged. Making the two atomic would mean threading a caller-owned
  transaction through `EventBus::emit`, which owns its own.

## What this does not cover

- **Triggers were already right and were left alone.** They are event-first: the
  HTTP handler emits `TriggerCreated` and the scheduler materializes state from
  the event, so there is no direct write to forget an emit for. That is the
  stronger pattern; this ADR is the retrofit for surfaces not built that way.

  **Amended 2026-08-03.** Event-first, yes, but that says nothing about *when*
  the materialization becomes visible, and the answer was "whenever the
  scheduler's subscriber task next ran". A write returned while the in-memory
  trigger registry still held pre-write state, so a `PUT {"paused": true}` that
  answered `success: true` could be followed by a run request that fired the
  trigger it had just paused. Triggers now go through a write chokepoint that
  emits and then applies the registry projection before returning
  (`engine/trigger_writes.rs`, mirroring `trigger_group_writes.rs`), which makes
  the surface read-your-writes without changing who owns the emit. The
  reachability-not-atomicity boundary below is unchanged: the event commits
  first, and the apply follows it.
- **Two `data/` helpers still announce at the call site.**
  `write_batch_and_commit` (a bulk import announces once as
  `RepositoryImported`, not once per file) and `commit_data_path` (a commit-only
  helper whose caller did its own write, so the manager cannot know whether the
  file pre-existed). Both are registered exemptions with removal conditions in
  `docs/temporary-measures.md` rather than left silent.
- **`email_accounts`, knowhow and `data/config/` stay `Silent`**, each with its
  reason on the registry entry, so the next person re-decides instead of
  re-discovering.
