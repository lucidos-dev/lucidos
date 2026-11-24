---
name: ThreadEvent Reference, Thread lifecycle (continued)
description: Part of ThreadEvent Reference. Its index lists the sections it covers.
---

One part of `system-knowhow/thread-events`, which lists every part.

## Thread lifecycle (continued)

### Deleting a thread is a `engine::event_bus::SystemEvent`, not one of these

A thread's own events cannot record its deletion, because the delete removes
them. So `ThreadsDeleted` is a **`engine::event_bus::SystemEvent`** on aggregate `ops` with
`aggregate_id` `global`. It is persisted and subscribable like any other, and
`condition: { thread_id: ... }` does NOT reach it: the row belongs to no thread.
Subscribe on the type and read `thread_ids` from the payload instead.

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `ThreadsDeleted` | The owner deleted a thread and every sub-thread under it, from the Lucidos UI. Carries `thread_ids` (the whole family, target first), `event_count`, `memory_count`, `worktrees_removed`, `widgets_removed` (the family's own non-reusable widgets, removed from `data/apps/` in one commit), optional `actor`. **No title, no message text, no summary**: it is the only record left, and it must not re-file what the delete removed. | lifecycle (rare) | yes | yes |

Deleting is not archiving. *Archive* moves a thread to the Archive section and
changes nothing about retrievability. Delete removes the rows, and what Lucidos
learned from them, with no undo. It is offered to the workspace owner in the UI
and to nobody else, so no tool, CLI verb or SDK method can reach it.

### Correcting a memory is a `engine::event_bus::SystemEvent` too

A memory correction belongs to no thread, so `MemoryCorrected` is a
**`engine::event_bus::SystemEvent`** on aggregate `memory` with `aggregate_id` `global`.

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `MemoryExtractionFailed` | Memory extraction exhausted retries or its deadline for one source. Carries `source`, `model` and `reason`: `call_failed`, `unreadable_reply` or `timed_out`. Recorded once per failed indexing operation, before raw-text fallback or artifact skipping. | per-failure | yes | yes |
| `MemoryCorrected` | The agent corrected a long-term memory at the user's word, through the `memory` tool's `correct` or `correct_by_id` action. Carries `search_query`, `wrong_fact`, `removed` (each deleted entry's `summary`, `entities` and `source`), an optional `correction` (the fact that replaces it), and `recorded_at`. `lucidos memory correct-by-id` emits it too, with its caller as `actor`. Every memory rebuild replays it, which is what keeps a corrected fact from coming back. | per-action (rare) | yes | yes |

The Tree *memory module*'s one-time backfill reports on aggregate `memory`
too. Settings → System → Memory draws its progress bar from these.

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `TreeBackfillProgressed` | The backfill moved: it started, a tree began or wrote a summary, one more thread (or the workspace) caught up, a tree failed, or it started or stopped waiting for a background model. Carries `progress`: `done`, `total`, `done_milli`, `nodes_done`, `nodes_total`, `waiting_for_model`, `retrying`, `ready`, `spent_usd`, `cap_usd`, `next_cap_usd`, `held` and `seq`. `seq` is the frame's order, rising while the engine runs. `done_milli` is `done` in thousandths plus each tree under way's built share. `ready` is true once turns use the Tree module while older threads still fill in. `spent_usd` is what the backfill has spent so far; `held` is true while it waits at its spending cap. | per thread during a backfill | no | no |
| `TreeBackfillStarted` | A backfill began. Carries `estimate` (the compactor `model` and `effort`, the `calls` range, and the `cost` row the Start dialog shows, absent for a model with no price), `cap_usd` (where it pauses, absent with no price) and `actor` (who chose Tree). | once per backfill | yes | yes |
| `TreeBackfillCompleted` | The workspace tree and the threads active in the last 7 days are built, so turns now use the Tree module. Older threads keep filling in after it. Carries `total`, the trees the backfill covers, and `spent_usd` so far. | once per backfill | yes | yes |
| `TreeBackfillCapReached` | The backfill spent its cap, so it paused. Threads it already summarized keep up with new messages; the rest wait. Carries `spent_usd` and `cap_usd`. The user gets a notification. | rare | yes | yes |
| `TreeBackfillCapRaised` | The user pressed Continue, moving the cap by half the estimate. Carries `cap_usd` and `actor`. | rare | yes | yes |
| `TreeBackfillEnded` | Every tree is built. Carries `spent_usd`, the backfill's whole cost, absent for a model with no price. | once per backfill | yes | yes |
| `TreeBackfillReset` | Switching back to Classic cleared a finished backfill, so choosing Tree again catches up first. | rare | yes | yes |

A provider refusing a background task's model reports on aggregate `ops`.

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `ModelNotServedObserved` | A provider answered that it does not serve the model a background task ran on: retired, never enabled, or misspelled. Carries `model`, `provider`, `purpose`, `message` (the provider's failure, with advice where Lucidos has one) and `moved_to`, the model the call retried on. No `moved_to` means the call failed. For six hours an unset default passes over the model; a stored pick keeps failing and gets one notification. | at most once per provider, model and task per six hours | yes | yes |

### A dashboard reports as `engine::event_bus::SystemEvent`s

The two *dashboards* belong to the workspace, not to a thread, so their events
are **`engine::event_bus::SystemEvent`s** on aggregate `dashboard`. The `aggregate_id` and the
payload's `dashboard` field are both `phone` or `desktop`. Each carries an
optional `actor`. The engine folds them on read, on top of the seed from
Home's old pins and `HomeBoardArranged` rows (ADR 0496).

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `DashboardTileAdded` | A *widget instance* became a tile on one dashboard, or an app became an *app shortcut* at `quarter`: Add to dashboard or a dashboard's own Add, `widgets(action="add_to_dashboard")`, or `lucidos widgets dashboard-add`. Carries `dashboard`, `app_id`, and the optional `params`, `label` and `size`. A widget tile starts at `size`, the size picked on the card it came from, else at its widget's default size. An app shortcut takes no `params`. A widget that belonged to its thread was made reusable first, which commits its manifest. | per-action | yes | yes |
| `DashboardTileRemoved` | A tile left one dashboard: Remove from dashboard, `widgets(action="remove_from_dashboard")`, or `lucidos widgets dashboard-remove`. Carries `dashboard`, `app_id` and the optional `params`. The other dashboard keeps its own tiles, and no file changes. | per-action | yes | yes |
| `DashboardArranged` | One dashboard was arranged: by a drop or a size change in edit mode, by `widgets(action="arrange")` when the user asked, or by `lucidos widgets arrange`. Carries `dashboard` and `tiles`, in order: each `app_id`, optional `params` and `size`, one of `quarter`, `small`, `medium`, `large` or `xl`. A widget takes the last four and an app shortcut only `quarter`. The manifest says which a tile is, so no event stores a kind. An entry naming no tile on that dashboard is ignored, and a tile it leaves out keeps its place after the named ones. | per-action | yes | yes |

### Freeing disk space reports as `engine::event_bus::SystemEvent`s

The *recommended cleanup* behind Disk Usage's "Free up space" button walks
every worktree, so its report belongs to no thread. Its four events are
**`engine::event_bus::SystemEvent`s** on aggregate `ops` with `aggregate_id` `global`. Each tree
it touches still gets its own `WorktreeCleaned` thread event.

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `RecommendedCleanupStarted` | The user started a pass. Carries optional `actor`. | per-action (rare) | yes | yes |
| `RecommendedCleanupProgress` | After each worktree the pass deals with, skipped ones included. Carries `done` and `total`. A transient frame like `BackupProgress`: subscribe to the terminal events instead. | per-worktree | no | no |
| `RecommendedCleanupCompleted` | The pass finished. Carries `removed_count` (finished worktrees removed), `cleaned_count` (worktrees whose build artifacts were cleared) and `freed_bytes`. | per-action (rare) | yes | yes |
| `RecommendedCleanupFailed` | The pass stopped before it finished. Carries `error`. | per-action (rare) | yes | yes |

## Planning stages

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `PlanStarted` | `lucidos planned start --kind design\|implementation` begins planning on the calling thread. Carries `kind`. Repeated starts of the same active kind record once, including concurrent calls and retries. A kind switch or intervening approval or landing begins a new stage. Grants no edit permission. | per-stage | yes | yes |
| `PlanProposed` | `planned mark --plan` on a calling thread records `kind`, `plan_path`, `plan_sha` (the committed git blob), and `branch_name`. Re-marking an already approved implementation plan can record an inherited approval instead. The marker and event commit atomically. | per-proposal | yes | yes |
| `PlanApproved` | The engine atomically records the marker and its evidence on the caller thread. Human approval checks the user's answer and proposed revision. The kinds `simple-fix` and `bounded-security-fix` evidence fix markers without a human approval or plan file. New rows name `branch_name`. Carries `kind` and `scope` (`implement` or `plan-only`). Plan revisions also carry `plan_path` and `plan_sha`. Design approvals always use `plan-only`. New implementation approvals include `repo_root` and `plan_version` (the SHA-256 of the plan after normalizing its status). Inherited approvals also carry `inherited_from_event_id`, pointing to the original approval decision. Concurrent or retried direct approvals record once. Replaying history never recreates marker permission. | per-approval | yes | yes |

`PlanStarted` with `kind: design` is evidence of Designing; with `kind: implementation`, Planning. It proves neither approval nor a landing. `PlanApproved` with `kind: design` proves Designed. With `kind: implementation, scope: implement`, it proves Implementing. An implementation approval with `scope: plan-only` still needs a plan-only landing to prove Ready to build. Match `plan_path` on proposal, approval, or handoff to a board item's `planPath`.

Plan approval checks the proposal revision attached to that repository's marker, even when one thread proposes the same path and branch in multiple repositories. Batch approval binds the latest session's canonical repository and branch. Legacy sessions without a repository can bind only an unambiguous marker; colliding branch names are refused.

