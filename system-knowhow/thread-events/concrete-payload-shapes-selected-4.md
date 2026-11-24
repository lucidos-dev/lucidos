---
name: ThreadEvent Reference, Concrete payload shapes (selected) (continued)
description: Part of ThreadEvent Reference. Its index lists the sections it covers.
---

<!-- domain-event: BuildBroken: Example domain event emitted by a build-monitoring script. -->
<!-- domain-event: OuraDataImported: Example domain event emitted by a wearable-data integration. -->

One part of `system-knowhow/thread-events`, which lists every part.

## Concrete payload shapes (selected) (continued)

### `TriggerStarted` / `TriggerCompleted`

```json
{
  "type": "TriggerStarted",
  "data": {
    "trigger_id": "trg-question-push",
    "trigger_name": "Push when Claude needs me",
    "prompt": null,
    "invocation": { "kind": "Event", "event_type": "UserQuestionAsked", "event_id": "…", "thread_id": "…" },
    "origin": { "kind": "engine", "reason": { "kind": "scheduler", "trigger_id": "trg-question-push", "trigger_name": "Push when Claude needs me" } },
    "go_to_review": false
  }
}
```

```json
{
  "type": "TriggerCompleted",
  "data": {
    "trigger_id": "trg-question-push",
    "trigger_name": "Push when Claude needs me",
    "result_summary": "sent push notification"
  }
}
```

Reads still accept the legacy `task_id` / `task_name` aliases; new emissions use `trigger_id` / `trigger_name`.

`TriggerStarted.continue_key`, when present, is the key the fire claimed for a new thread (`system-knowhow/triggers/writing-cron-expressions.md` § "Continue key"). A fire that continues an open thread emits its own `TriggerStarted` there without one, and the thread keeps its key.

`origin.reason.actor_email` on a `scheduler` origin, when present, is the email the trigger's `actor` field resolved to (`system-knowhow/triggers/actor.md` § "Actor"). It rides on `TriggerStarted` and on the answer a keyed fire gives to a question. A display credits the user holding that email.

### `TriggerRunInterrupted`

Boot writes this event on a trigger root after an engine restart cut its turn.
It restores the inbox without changing the abort's failed status, the thread's recency, or its pin.
Human follow-ups, parked questions, sub-threads, and turns that auto-resumed are excluded.

`trigger_id` and `trigger_name` identify the trigger.
`fire_event_id` identifies the cut fire's `TriggerStarted` event.
`retry` records the durable decision:

- `{ kind: "queued", entry_id, deadline }` identifies a fresh fire in the Thread Queue.
- `{ kind: "skipped", reason }` records why no fresh fire was queued.

Skipped reasons are `already_retried`, `fire_completed`, `spawned_work`, `too_old`, `later_slot_due`, `newer_fire`, `trigger_unavailable`, `request_unavailable`, and `already_queued`.
A fire whose main work finished has `fire_completed`; Continue resumes its later work.
The notification opens the cut thread.

`TriggerStarted.retry_of`, when present, carries `{ thread_id, deadline }`.
It points back to the cut thread and bounds admission of the fresh fire.
The transcript places the interruption notice beneath the abort, and the retry header links to the cut thread.
Queued does not promise execution: expiry or a second cut drops the entry with a notification.
Older events omit it and mean an original fire.

Lucidos no longer posts trigger grant review cards: a trigger grant ask replaced them.
A review card still open from an older version carries a `reviewed_trigger_program_grants` batch item with `selectable: true`.
Each option ID names one proposed pattern. A `MultiSelected` answer names the subset to save, including an empty subset.
The review receipt records the subset. Older review items keep their original Allow or Deny behavior.

On a trigger blocked by the command guard, `UserQuestionAsked.permission_request.batch.items` can carry a `trigger_program_grant` item.
Its fields are `trigger_id`, `definition_digest`, `pattern`, `queue_entry_id` and `failed_thread_id`.
An owner grant persists the program grant before queuing the original request with `retry_of`.

A detected sandbox write refusal can carry a `trigger_folder_grant` item.
In place of `pattern`, its `grant` names the folder and its filesystem identity.
The engine rechecks the folder and the definition digest after the owner answers the permission request.
The answer settles the original thread as failed.

### `WorktreeCleaned`

```json
{
  "type": "WorktreeCleaned",
  "data": {
    "tier": 2,
    "freed_bytes": 4823104
  }
}
```

- `tier: 0`: a finished worktree (clean, branch at main HEAD, no pending change) removed. The background worker waits out a short grace window first. The Disk Usage page's recommended cleanup removes it at once, on the user's request.
- `tier: 1`: build artifacts (`target/`, `node_modules/`, `.lucidos/cache/`) stripped from an idle worktree, which stays on disk. This happens, for example, an hour after its change was applied.
- `tier: 2`: the entire worktree directory removed. That covers an archived thread's worktree, an hour after it idles; any uncommitted work there is committed on the thread's branch first. It also covers the 30-day idle sweep and *stranded* worktrees. A stranded worktree's git admin dir under `.git/worktrees/<name>` is gone, so git cannot act on it.

Reclamation keeps the thread branch. The wire omits `branch_deleted` when it is false.
Historical events can have `branch_deleted: true` if cleanup also dropped a fully merged branch.

### `ContinuationStarted`

```json
{
  "type": "ContinuationStarted",
  "data": {
    "branch": "lucidos-claude-code-repo-lucidos-…",
    "origin": { "kind": "engine", "reason": { "kind": "continuation_started" } },
    "reason": "auto_recovery_after_hang"
  }
}
```

`origin.reason.kind` is `continuation_started` (legacy alias `session_recovered`).

`reason` (optional) mirrors the originating `ContinuationRequested.reason`, so
the timeline names which interruption the resume recovered from:

- `user_clicked_continue`: a genuine resume after an engine restart (the user
  tapped "continue").
- `harden_requested`: the user pressed **Harden** on the Not ready strip, and
  the thread resumed to run `/harden`. Its turn end proposes the change.
- `plan_requested`, `build_requested`, `handoff_requested`,
  `reports_requested`: the thread's automation is paused, and the owner let
  one step run from its permission request. The resume carries the plan
  nudge, the order to build the approved plan, the context-ceiling handoff,
  or a wake to read the reports filed meanwhile.
- `auto_recovery_after_hang`: a hung subprocess OR a stray signal-kill where
  **nothing restarted**, e.g. another workspace's `cargo check` build-lock kill
  hitting this agent's process. The UI labels it "Resumed after the session
  stopped responding".
- `auto_resume_after_api_error`: the engine resumed after a transient upstream
  failure the agent reported itself. Labelled "Resumed after the model
  connection dropped".
- `effort_restart`: older threads only. The user picked a new reasoning effort
  on a Claude Code thread mid-turn, and the engine resumed the session at the
  new effort. Nothing emits it now (ADR 0542). Labelled "Restarted at the new
  reasoning effort".

Neither auto reason claims "Resumed after engine restart". The two are worded
apart because both can fire on one thread minutes apart. `reason` is absent on
legacy rows and the chat-rerun path.

`resumed_abort_id` (optional) names the `ResponseAborted` a chat or trigger
Continue resumes. A unique index allows one `ContinuationStarted` per abort, so
a double Continue opens one boundary and one rerun. It is absent on the
coding-agent path, on the answer-driven fallback, and on legacy rows.

**The boundary confers no thread type.** `ContinuationStarted` is emitted on all
three channels. `chat` and `trigger` come from `emit_resume_anchor` (reached from
`POST /api/v1/threads/<id>/continue` and the chat answer-resume path), and
`claude_code` from the coding agent's `--resume` dispatch. A thread is a
*coding-agent thread* because a `SessionStarted` opened an agent session on it,
never because it was continued. So the `thread_summaries` projection sets
`is_coding_agent` (and rewrites `source`) for every `SessionStarted`, but for
`ContinuationStarted` only on the `claude_code` channel. It never *clears* the
flag; repairing rows corrupted by an earlier write is a migration's job.

**Only an interrupted turn gets this boundary.** A thread parked on an
unanswered `UserQuestionAsked` is *preserved* across a restart (no
`ResponseAborted`). So answering it emits **no** `ContinuationStarted`: the
resumed work carries the original turn's `request_event_id` and continues that
exchange. A subscription on `ContinuationStarted` therefore fires for a revived
interruption, never for an answered question. "Parked" holds only while the
question is the newest thing on the thread.

If anything in `ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES`, or the question's own `CodingAgentToolResult`, landed after it,
the card is dead. The thread then recovers as an ordinary interrupted turn, with
its boundary and its Continue button. See `coding-agent-events.md`
§ "An engine restart alone does NOT orphan a pending question".

## How a workspace would actually trigger on these

Any persisted `ThreadEvent` outside the blocklist (the four per-token streaming variants and the four side-question events) takes an `on` subscription directly:

```yaml
on:
  - event_type: ChangeApplied
    condition:
      hardened: true
run:
  intent: "Tell me which change just landed and what files it touched."
```

Four knobs:

1. **Pick the right event.** Lifecycle and one-per-turn variants are usually what you want. Always pair a per-action variant with a `condition:` filter (§ "Volume classes").
2. **Per-token streaming is off-limits**, and the create is refused. For token-level reactivity, consume the SSE stream directly.
3. **For workspace-defined signals**, `lucidos events emit` (or the `events(action="emit")` LLM tool) writes a `SystemEvent::DomainEvent` that always reaches the matcher. Use it for a name outside the engine's ThreadEvent enum (e.g. `OuraDataImported`, `BuildBroken`); see `system-knowhow/lucidos-cli.md`.
4. **For workspace-scoped engine facts**, subscribe to the persisted `engine::event_bus::SystemEvent` directly. `BackupFailed`, `NotificationCreated` and `TriggerCompleted` are `on:` entries like any other. Never emit a domain event beside one to make it reachable: the engine already wrote the row.

Trigger-run failures auto-create an error notification, so "tell me when one of my triggers blew up" needs no wiring.

## Recipe-shaped guidance

Trigger config syntax (cron format, the `on` list, the per-entry `condition` operators): `system-knowhow/triggers.md`. Conditions are pure payload filters: each key is a field path into the event payload (the `data: { … }` object above).

The coding-agent slice (the `UserQuestion` vs permission distinction, the exact `CodingAgentIdled` field semantics, the no-`CodingAgentErrored` gap): `system-knowhow/coding-agent-events.md`.

Event-store column shape (`event_type`, `payload`, `created`, `aggregate`, `aggregate_id`, `sequence`) and the queries that walk threads from events: `.claude/rules/db.md`.

### Workspace snapshot events

Local workspace snapshots announce `SnapshotCreated`, `SnapshotCaptureFailed`,
`SnapshotChecked` and `SnapshotDeleted` as persisted system events. They carry
`workspace_id`, an opaque `handle` and the initiating `actor`. Capture/check
outcomes also carry `job_id`; check outcomes include `passed`. Capture failures
use fixed reason codes: `capture-failed`, `interrupted` for jobs settled at startup,
and `disk-floor`. A `disk-floor` reason is the object
`{"disk-floor":{"needed_bytes":N}}`, where `N` is the space to free.

Deletion reasons distinguish `manual` from `retention`. Retention announces each
successful deletion before attempting another. These events use the `snapshot`
aggregate and the handle as aggregate identity, with no path or key material.

`SnapshotProgress` is transient capture/check activity, with the job's `action`.
It is broadcast before the terminal event and cannot drive a trigger or event
wait. Subscribe to `SnapshotCreated`, `SnapshotCaptureFailed` or `SnapshotChecked`
for durable outcomes.
