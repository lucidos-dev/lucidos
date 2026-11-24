---
name: ThreadEvent Reference, Concrete payload shapes (selected) (continued)
description: Part of ThreadEvent Reference. Its index lists the sections it covers.
---

<!-- domain-event: LucidosReleased: Release automation announces a published version. Optional `commits` lists source main-line merges since the previous release. -->

One part of `system-knowhow/thread-events`, which lists every part.

## Concrete payload shapes (selected) (continued)

### `ToolResult`

```json
{
  "type": "ToolResult",
  "data": {
    "name": "run_bash",
    "result": "file1.txt\nfile2.txt\n",
    "images": [],
    "tool_called_event_id": "8b1d3e0a-7c0b-4e2f-9c4a-1a2b3c4d5e6f"
  }
}
```

`result` is the textual output, such as bash stdout or file contents. `images` carries generated-image hashes that render inline in the chat exchange. `tool_called_event_id` names the `ToolCalled` it answers, on every live emit and on backfills from `recover_orphan_tool_calls`. It is the only reliable pairing key, since a batch of pure reads answers in completion order. Every engine reader and the frontend's step rows pair by it, falling back to position only for legacy rows.

**Snapshot endpoint strips the heavy field, for both channels**, like `ContextCaptured` above. `GET /api/v1/threads/:thread_id/events` removes `result` from every `ToolResult` and `CodingAgentToolResult` and stamps `result_stripped: true`. One bash result can be 150 kB. Only the step detail (`StepDetailModal.tsx`) renders a result. The strip keeps `name`, `images` and `tool_use_id`, which the step label, the image paths and the pairing need.

To fetch the dropped text, call `GET /api/v1/events/:event_id/tool-result`. It returns `{ result: string | null }`, with `null` for an image-only result. `?include_context=true` on the snapshot opts back into `result` as well as `ContextCaptured.sections`; `exportThread.ts` uses it.

On the live SSE stream a `ToolResult` carries its full text, and a `CodingAgentToolResult` arrives already stripped. See `CodingAgentToolResult` in [coding-agent-events](../coding-agent-events.md) for why.

**A tool call's `args` are stripped everywhere a client reads.** The snapshot and the live SSE stream both remove `args` from every `ToolCalled` and `CodingAgentToolCalled` and stamp `args_stripped: true`. Otherwise a `Write` would send a whole file to every client. `description` is filled first, so the step label never needs the args. A `generate_image` call keeps its args, because the image takes its alt text from the prompt.

To fetch the dropped args, call `GET /api/v1/events/:event_id/tool-args`. It returns `{ args }` for that one call. The stored row keeps them, so a trigger condition on `args.command` still matches, and `events(action="query")` still returns them.

### Plan and implementation stages

A factory can use `ChangeApplyRequested` to timestamp the start of Landing.
Today Apply records the request and continues through its normal landing route.
`PlanApproved.branch_name` identifies the approved branch. Fix marker kinds
show that editing could start without a human approval.

Use engine facts, matched by `plan_path` on `PlanProposed`, `PlanApproved`, or
`SessionHandedOff` to the board item's `planPath`:

| Board stage | Evidence |
|---|---|
| Designing | `PlanStarted` or `PlanProposed` with `kind: design` |
| Designed | `PlanApproved` with `kind: design` |
| Planning | `PlanStarted` or `PlanProposed` with `kind: implementation` |
| Plan awaiting Apply | `ChangeProposed` with `content: plan-only` |
| Ready to build | `PlanApproved` with `kind: implementation, scope: plan-only`, followed by `ChangeApplied` with `content: plan-only` |
| Implementing | `PlanApproved` with `kind: implementation, scope: implement` |
| Ready to land | `ChangeProposed` with `content: code` |
| Landed | `ChangeApplied` with `content: code` |
| Released | `LucidosReleased` for the code landing |

`content` comes from the proposed files or actual landed Git trees through the
plan-file classifier, never from approval state. A mixed diff is `code`; an empty
landing is also `code`, so a no-op apply cannot prove a plan landed. Historical
change events without `content` deserialize as `code`; they provide no evidence
of a plan-only landing. Approval binds the committed `plan_sha`, so stage
projections must keep the approval and landing associated with the same plan.

### `ChangeProposed`

```json
{
  "type": "ChangeProposed",
  "data": {
    "content": "code",
    "change_id": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
    "description": "Add ThreadEvent reference doc",
    "files": ["system-knowhow/thread-events.md"],
    "branch_name": "lucidos-claude-code-repo-lucidos-add-threadevent-reference-doc",
    "repo_root": "/Users/me/workspaces/dev"
  }
}
```

Except for `change_id` and `content`, fields are omitted at their zero value: `false`, `null`, an empty string or an empty list. So `requires_restart`, `hardened`, `incomplete`, `set_aside` and `origin` appear only when set. Read them defensively, as `payload.incomplete ?? false`. A live emit has no `commit_sha` key: the engine always sets it to `None`. Only legacy per-commit rows carry one.

Several events with the same `change_id` can arrive for a branch, one per proposing turn. The projection in `core::changes_projection` folds them into a single `changes` row.

`set_aside: true` proposes straight into set-aside, and leaves an archived thread archived. The engine does that in three cases, once per branch, and never once a change row names the branch:

- Work on an archived thread's branch, with origin reason `archived_branch_work`. It runs on the archive, after a full worktree removal, or on a boot pass. A removal may have committed the tree's dirt there.
- An idle *integrated sub-thread*'s work, when its parent's change lands without its branch. The origin reason is `unmerged_integrated_work`. The proposal clears the sub-thread's `integrated_into_parent` reason.
- Work a thread committed after it handed off to a *legacy successor thread*, when the successor's change is applied or discarded without it. The origin reason is `successor_residue`. Work the successor's branch carries is never set aside, and only a change row recorded since the handoff, or an open one, stops it.

`incomplete: true` marks a set-aside proposal whose turn nobody saw finish. The archive net always sets it, since it cannot tell. Older rows can carry it on a pending proposal from a Stop. `description` is optional, and old rows also carry legacy `path` and `diff`. The legacy per-commit shape (empty `change_id`, `commit_sha` set) is inert in the projection.

### `ProposalWithheld`

Optional `agent_returns: true` means the engine will send the agent back for this hold.
The event keeps its reason, but the summary carries none: the owner owes no action yet.
Omitting it means the work waits on the owner.
If re-entry fails, the engine announces the hold again without this flag.


<!-- generated:begin UnproposedReason -->
UnproposedReason wire values:

- `plan_missing`
- `plan_changed`
- `plan_unreadable`
- `plan_awaiting_approval`
- `plan_only_code`
- `outside_bound`
- `hardening_missing`
- `turn_incomplete`
- `awaiting_landing`
- `integrated_into_parent`
- `carried_by_successor`
<!-- generated:end -->

`plan_changed` means the committed plan changed after approval. Restoring the approved text reuses the approval.

`plan_unreadable` means Lucidos could not verify the plan marker or its committed version. Retry the check.

`plan_only_code` means approval for later covers plan files alone, while the branch also changes code.
The agent must remove those code changes or obtain implementation approval.

`integrated_into_parent` marks an *integrated sub-thread*'s hardened work, which its parent's agent merges.
The child's work is never proposed on its own.
`carried_by_successor` marks the thread that handed off to a *legacy successor thread*, whose change carries the work.
`awaiting_landing` means the thread's change waits in the landing lane.

```json
{
  "type": "ProposalWithheld",
  "data": {
    "branch_name": "lucidos-claude-code-repo-lucidos-add-threadevent-reference-doc",
    "files": ["system-knowhow/thread-events.md"],
    "reason": "turn_incomplete"
  }
}
```

A trigger for "a coding agent left work it did not propose" subscribes to `ProposalWithheld`. Add `condition: { "reason": "turn_incomplete" }` for stopped or failed turns only.

### `ChangeWithdrawn`

```json
{
  "type": "ChangeWithdrawn",
  "data": { "change_id": "7c9e6679-7425-40de-944b-e07fc1f90ae7" }
}
```

The `changes` row keeps its id with status `withdrawn`. A later proposal for the same branch gets a fresh `change_id`.

### `ChangeApplied`

```json
{
  "type": "ChangeApplied",
  "data": {
    "content": "code",
    "change_id": "chg-2025-05-13-…",
    "requires_restart": false,
    "client_update": false,
    "commits": ["docs: add thread-events reference"],
    "thread_title": "Document all ThreadEvents",
    "actor": { "kind": "device", "device_id": "device-abc123" },
    "pre_merge_sha": "9b38db1b4…",
    "post_merge_sha": "a1b2c3d4e…",
    "landing_merge_sha": "a1b2c3d4e…",
    "path": ""
  }
}
```

A change the landing lane landed has `post_merge_sha` equal to `landing_merge_sha`, and `pre_merge_sha` is that merge's first parent.

**At most once per `change_id`.** Every apply path emits `ChangeApplied` through `EventBus::emit`, which `FOR UPDATE`-claims the change row and suppresses the emit if the row is already `applied`. So two racing applies, an HTTP or Apply-All retry, a conflict-recovery cleanup or a post-restart re-apply never persist a second one. The timeline shows one "Change applied" entry per change. Recovery no-ops and the external-repo archive carve-out run while the row is still `pending` (or absent), so they still emit exactly once.

### `ChildThreadCompleted`

```json
{
  "type": "ChildThreadCompleted",
  "data": {
    "child_thread_id": "550e8400-e29b-41d4-a716-446655440000",
    "child_thread_title": "Sub-task: rename foo to bar",
    "status": "success",
    "summary": "Renamed all 14 occurrences across 9 files. Tests pass.",
    "pending_change_ids": ["chg-2025-05-13-…"],
    "sub_thread_pending_changes": [
      {
        "change_id": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
        "thread_id": "6fa459ea-ee8a-3ca4-894e-db77e160355e",
        "thread_title": "Sub-task: update the call sites",
        "thread_unsettled": false
      }
    ]
  }
}
```

`status` is `success` / `failure` / `no_changes` / `canceled` / `interrupted`. `canceled` means the user ended the child: they archived it, discarded its change, or an Apply, Discard or Archive cut its running turn short. A user Stop is never `canceled`: it sends `ChildThreadStopped`. `summary` is truncated to 2000 chars. `pending_change_ids` is empty for chat children and for coding-agent children that proposed nothing.

`interrupted` means an engine restart cut the child's turn and nothing will resume it: a crash, or a shutdown the user did not ask for. The child's work is kept, and a follow-up continues it. The user's own *Switch to new version* sends no card: the engine resumes the child, and the resumed turn reports.

**Two lists, kept apart: the child's own changes and its sub-threads' changes.** `pending_change_ids` names only the child's own branch. An orchestrating child whose children did the work holds none, so that list is empty while work waits below. `sub_thread_pending_changes` lists every change pending anywhere below the child, whatever its status. Each entry names its owner (`thread_id`, `thread_title`) and `thread_unsettled`, as in the `changes` list: `true` if that sub-thread was still working when the card was sent. It is a snapshot, so read the `changes` list before you apply.

The conversation block reads `Pending changes: none` for the child's own branch, then a `Pending changes in its sub-threads` section when this list is non-empty.

The payload is frozen, but the block is not. Each time the engine rebuilds the block, it reads every listed change's status again. A change that left Review since the report gets a mark: `(now applied)`, `(now discarded)`, `(now reverted)`, `(now set aside)`, `(now withdrawn)` or `(now deleted)`. A note then says it no longer waits for Apply. Trust that mark over the child's summary and over anything you said earlier.

**The parent is re-opened BY this callback, so it never has to wait for one.** The fan-in persists it on the parent and re-opens that thread with the status, summary and `pending_change_ids` an *event wait* would deliver. A report its *wake mode* files arrives as a `ChildReportFiled` instead, which re-opens nothing.

So an `await_event` (or `lucidos await-event`) on your own child's completion is redundant. The engine stands the callback down when a live wait covers it, so it is one turn either way. But the wait still spends recent-subscription budget, and its timeout can fire while the child works.

Await a `ChildThreadCompleted` only for a completion that is not your own child's, named with a `child_thread_id` condition. Matching is workspace-wide, so it can be any thread's child. The card lands on whichever thread is the parent, and the wait resolves off that row. Name `ChildReportFiled` in the same `on` list too, since the report may be filed instead.

**One callback per completed turn, not one per child.** A *child follow-up* revives or redirects the child, and that turn's terminal sends a second `ChildThreadCompleted` for the same `child_thread_id`, on the same parent. A human clicking Continue on a coding-agent child does the same. So `child_thread_id` is not a key; the events are a log of completed turns.

These are **not completions**, and fire no card:

- **A steer.** A `ResponseCanceled` with cause `superseded_by_followup` is the mid-turn redirect when a follow-up lands on a live Codex turn. Cause `effort_restart`, on older threads, is the same. The `CodingAgentIdled` after that cancel fires no card either. The child runs the redirected turn at once, and that turn's terminal is the report.
- **A terminal the engine is about to resume.** A coding-agent turn that dies on a transient upstream `API Error` emits a real `ResponseFailed`, and the engine resumes the session seconds later. That `ResponseFailed` and its `CodingAgentIdled` fire no parent callback, so the parent sees ONE card for the episode. The suppression depends on the resume being scheduled, never on the error. Past `MAX_API_ERROR_AUTO_RESUMES` the thread parks for good and the `failure` card fires (ADR 0199).
- **A user Stop.** A Stop on the child's running turn, or Cancel on its question card, ends the turn but not the child. The parent gets a `ChildThreadStopped` note instead, and no turn runs. The child's next finished turn sends this card as usual.
- **A turn that ends holding an event wait.** The wait wakes the child for another turn, and that turn reports.

If the user archives, deletes, or discards the child's change, this card arrives with status `canceled`. Until then the child counts toward the user's attention. A cancel you issue on your own child is not the user's Stop, and still arrives as a `canceled` card (ADR 0252).

Stop waiting and an agent standing its wait down wake nothing. So when an idle child's last wait ends that way, the card it held back arrives then. Archiving a child that holds a wait sends a `canceled` card. A failed turn reports at once, wait or not (ADR 0254).

**Running more than one child at a time: `system-knowhow/orchestrating-sub-threads.md`.** This edge and the *child follow-up* are the only two carrying traffic between threads; nothing carries it sideways. That file is the manual for a parent coordinating several children. It covers what a child may do about a sibling's events, and how a ruling reaches a child that already finished.

### `ChildReportFiled`

An integrated sub-thread's repeat report can also file when its hardened head
and report content match its last report to the parent. An explicit wake
request and the final wake for a batch take precedence.

Same payload as `ChildThreadCompleted`, under its own type:

```json
{
  "type": "ChildReportFiled",
  "data": {
    "child_thread_id": "550e8400-e29b-41d4-a716-446655440000",
    "child_thread_title": "Sub-task: check the venue's hours",
    "status": "success",
    "summary": "It opens at nine on weekdays."
  }
}
```

A child's *wake mode* filed its report here instead of waking this thread. A `no-wake` child files every report, whatever its `status`: a failure, a cancel and an `interrupted` turn too, since the child shows those itself (ADR 0545). A `wake-when-all-done` child files a clean report while a sibling of its batch still works. A child with no mode files a clean report when its *wake decision* said no. A parent that needs the result spawns with `wake`. Under every other mode, a failure, a cancel, an `interrupted` turn, a cut-short turn and an empty reply send a waking `ChildThreadCompleted`.

It never re-opens the parent, moves no status and no drawer row, and the boot sweep never re-fires it. It does settle what the child owed, as the card would. A chat parent reads it as a `[CHILD THREAD REPORT FILED]` block in its history, with a note that it woke nothing. A coding-agent parent reads it whole in the turn-gap note at the front of its next prompt, once.

### `ChildThreadStopped`

```json
{
  "type": "ChildThreadStopped",
  "data": {
    "child_thread_id": "550e8400-e29b-41d4-a716-446655440000",
    "child_thread_title": "Sub-task: rename foo to bar"
  }
}
```

A user Stop paused one of this thread's children. The child is **alive**: it is waiting for the user, who may send it a message. Do not roll back its work, respawn it, or send it a follow-up on the strength of this note. Exactly one of three things follows:

- The user continues the child. Its next finished turn reports with the real status.
- The user archives or deletes the child, or discards its change. A report with status `canceled` arrives.

Either report is a `ChildThreadCompleted`, or a `ChildReportFiled` when the child is `no-wake`.
- Nothing, for as long as the user leaves it. The child stays a *stopped child* and counts toward their attention.

It never re-opens the parent. A chat parent reads it as a `[CHILD THREAD STOPPED]` block in its history. A coding-agent parent reads it in its turn-gap note. To be told when the child is really done, rely on the `ChildThreadCompleted` fan-in; a wait on `ChildThreadStopped` fires on the Stop itself.

### `ChildThreadDetached`

```json
{
  "type": "ChildThreadDetached",
  "data": {
    "child_thread_id": "550e8400-e29b-41d4-a716-446655440000",
    "child_thread_title": "Sub-task: rename foo to bar"
  }
}
```

One of this thread's children was moved to top level. It is no longer this thread's child, and it cannot be put back.

- **Nothing was stopped.** A turn in flight finishes, keeps its work and proposes any change. Its result lands on its own timeline only.
- **This thread gets nothing more from it.** No `ChildThreadCompleted`, no `ChildThreadStopped`, and no follow-up: `follow_up_child_thread` refuses it as not your child. `my_children` no longer lists it.
- **It holds its child slot while it runs.** The parent's cap counts a moved child until it finishes, so a move never makes room for another spawn while the child still works.
- **A card the child earned before the move still arrives.** Only what happens after the move is cut.

It never re-opens the parent. A chat parent reads it as a `[CHILD THREAD MOVED OUT]` block in its history. A coding-agent parent reads it in its turn-gap note. A parent that armed its own `await_event` on the child's `ChildThreadCompleted` is not told, and that wait runs to its timeout.

The event lands on the parent so that ADR 0011's recovery checks still read the child's own latest event correctly. The engine drops a second move of the same child, so the event appears at most once per child.

