---
name: Apply Change API
description: How to apply CC-proposed changes from a thread worktree to main, verify success, and emit the ChangeApplied event
---

# Apply Change API

`POST /api/v1/changes/:id/apply` merges a CC-proposed change branch into `main`.
The response is a typed JSON body that makes verification self-contained —
callers no longer need to poll thread state to confirm "did this land,
and at what SHA."

## Endpoint

```
POST /api/v1/changes/{change_id}/apply
```

- Path: `change_id` — the UUID returned when CC proposed the change
  (`ChangeProposed` event payload, or `changes` table row).
- No request body.
- No query parameters.

## Response shape

Always JSON, with four HTTP statuses.

- `200 OK` on accepted requests, including `noop`, `hardening`, `conflict` and `queued`.
- `400 Bad Request` when the engine could not even consider the change (missing
  change, invalid status, and so on).
- `403 Forbidden` when the caller is a thread with no authority over the
  change's own thread (ADR 0168).
- `409 Conflict` when the owning thread's current state does not offer Apply.
  That is the server-side mirror of the UI gate, for a thread mid-turn or
  Running.

An idempotent re-apply of an already-applied change is NOT gated: it falls
through to the engine and returns `200` with `status: "noop"`.

```ts
type ApplyStatus = 'applied' | 'noop' | 'hardening' | 'conflict' | 'queued';

interface ApplyChangeResult {
  status: ApplyStatus;
  change_id: string;          // echoes the path param
  thread_id: string | null;   // owning thread; null for headless imports
  message: string;            // human-readable summary
  restart_required: boolean;
  applied_commit?: string;    // 40-char SHA on main AFTER the merge
  previous_commit?: string;   // 40-char SHA on main BEFORE the merge
  commits_applied: number;    // commits added to main (0 for non-`applied`)
  files_changed: number;      // files declared on the change
  conflict_thread_id?: string; // set when status === 'conflict'
  review_thread_id?: string;   // set when status === 'hardening'
}
```

### Status values

| Status | Meaning |
|---|---|
| `applied` | Branch merged into `main`. `applied_commit` and `previous_commit` are populated. The exception is the external-repo handoff path, which marks the change applied without merging — SHAs are absent there. |
| `noop` | Nothing to merge. Either the change was already applied (idempotent re-apply) or the branch had no commits. The original `applied_commit` is echoed back on idempotent re-apply, so callers can still reference the merge. |
| `hardening` | The change wasn't hardened, so a hardening recovery session was spawned. The change will auto-apply when hardening completes. `review_thread_id` points at the recovery thread. |
| `conflict` | Merge conflict — a CC session is resolving it; `conflict_thread_id` points at the thread to focus. For a **live session whose `main` has diverged** and for the **temp-worktree merge**, resolution runs in the background: the call returns immediately and the change auto-applies (`ChangeApplied`) when resolution completes, or emits `ChangeApplyFailed` if it can't. (A clean fast-forward on a live session stays synchronous and returns `applied` instead — only true divergence goes async.) A second apply while one is already running for a live session also returns `conflict` ("already in progress"). |
| `queued` | A Lucidos-source change waits in the *landing lane* (ADR 0463). It would not merge cleanly onto `main`, so it resolves in a lane batch. It waits for the next batch while one resolves, or for its *Apply All* press to hand over the rest (ADR 0480). `ChangeApplied` reports the landing, and `ChangeUnqueued` a change that left the lane without landing. A Lucidos-source change that merges cleanly lands at once and returns `applied`, with its own landing merge as `applied_commit`. |

### Canceling a lane batch

`POST /api/v1/changes/lane/cancel` with `{"batch_id": "<uuid>"}` fails the resolving lane batch whole and stops its lane thread. The batch id is on `LaneBatchStarted`. Every member stays pending and apply-ready, gets `ChangeUnqueued { reason: batch_failed }`, and waits for the next Apply. It answers `{"canceled": "<uuid>"}`, or `400` when no batch with that id is resolving. A caller without Apply's reach over every member is refused, as Apply refuses it.

### Removing a waiting change

`DELETE /api/v1/changes/<id>/queue` takes a change waiting in the landing lane back out. It gets `ChangeUnqueued { reason: removed }`, stays pending and apply-ready, and waits for the next Apply. It answers `{"message": "Removed from the landing lane."}`. A member of the open lane batch answers `409` with `"reason": "in_lane_batch"`: it leaves only with its batch, through the Cancel above. A change that is not waiting answers `409` with `"reason": "not_waiting_to_land"`, and an unknown id `404`.

### The main check

`GET /api/v1/changes`, every `ChangesUpdated` frame and `GET /api/v1/main-check` carry `main_check`, the *main check* (ADR 0464). A field a state has nothing for is absent:

```json
{
  "state": "red",
  "check_id": "<uuid>",
  "from_sha": "<sha>",
  "to_sha": "<sha>",
  "run_url": "https://github.com/lucidos-dev/lucidos/actions/runs/<id>",
  "fix_thread_id": "<uuid>",
  "summary": "Failed: engine."
}
```

`state` is `idle`, `running`, `passed`, `red` or `unavailable`. For `unavailable`, `summary` says why this checkout cannot run the check. `POST /api/v1/main-check/close` closes a red check for the maintainer and answers `{"message": "Closed the red main check."}`. It answers `409` with `"reason": "main_check_not_red"` when main is not red, and `403` with `"reason": "maintainer_only"` for any caller but the owner at a registered device: an agent, a script holding the machine-local token, or an app.

### The lane on `GET /api/v1/changes`

`GET /api/v1/changes` and every `ChangesUpdated` frame carry `landing_lane`:

```json
{
  "open_batch": {
    "batch_id": "<uuid>",
    "lane_thread_id": "<uuid>",
    "members": ["<change uuid>"],
    "step": "resolving"
  },
  "waiting": ["<change uuid>"],
  "gathering": false
}
```

`open_batch` is `null` while no batch resolves. `members` lists the members still to land. `step` is `landing` from the lane thread's clean finish until `main` moves, and `resolving` otherwise. `waiting` lists the changes waiting for the next batch, in landing order. After an engine restart all three stay empty until the lane is rebuilt.

`gathering` is `true` while an *Apply All* press is still handing changes over and no batch is open. What waits then forms one batch once the press is in.

## Examples

### Successful apply

```json
{
  "status": "applied",
  "change_id": "fbcc4a3a-2c14-4d5b-8d1a-9e84d4c9d4ec",
  "thread_id": "1c1c34ef-1f9e-4a9d-9c7e-c92f6b3fc1ea",
  "message": "Change applied.",
  "restart_required": false,
  "applied_commit": "9b1a3c5d8e7f2a1b4c6d9e8f7a3b5c2d1e9f8a7b",
  "previous_commit": "2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b",
  "commits_applied": 3,
  "files_changed": 5
}
```

### No-op (already applied)

```json
{
  "status": "noop",
  "change_id": "fbcc4a3a-2c14-4d5b-8d1a-9e84d4c9d4ec",
  "thread_id": "1c1c34ef-1f9e-4a9d-9c7e-c92f6b3fc1ea",
  "message": "Change already applied.",
  "restart_required": false,
  "applied_commit": "9b1a3c5d8e7f2a1b4c6d9e8f7a3b5c2d1e9f8a7b",
  "previous_commit": "2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b",
  "commits_applied": 3,
  "files_changed": 5
}
```

### Conflict

```json
{
  "status": "conflict",
  "change_id": "fbcc4a3a-2c14-4d5b-8d1a-9e84d4c9d4ec",
  "thread_id": "1c1c34ef-1f9e-4a9d-9c7e-c92f6b3fc1ea",
  "message": "Branch needs merging — agent is handling it.",
  "restart_required": false,
  "commits_applied": 0,
  "files_changed": 5,
  "conflict_thread_id": "1c1c34ef-1f9e-4a9d-9c7e-c92f6b3fc1ea"
}
```

### Error

```json
{ "error": "Change not found" }
```

Returned with `400 Bad Request`. The body always has an `error` key and no
`status` key — distinct from any successful outcome.

## Standard procedure

For automated callers (CC sessions, agentic loops, the assistant itself):

1. **CC commits in the worktree.** The proposed change must have at least
   one commit on the branch (uncommitted work is auto-committed by the
   apply path's recovery step, but committing explicitly is safer).
2. **Call the apply path.** Two surfaces:
   - **From inside a Lucidos-spawned subprocess (`run_python`, `run_bash`,
     scheduled scripts, CC sessions)** — use the `lucidos changes apply
     <id>` CLI. It auto-forwards the subprocess-origin headers (see "Actor
     attribution" below). Hand-rolling `urllib` / `curl` from the same
     contexts is a bug — see the example in `system-knowhow/lucidos-cli.md`
     § `lucidos changes apply`.
   - **From the frontend / a typed TS client** — call `applyChange()` in
     `crates/lucidos-app/src/api/client.ts` (which posts directly to the
     HTTP endpoint with the browser's device-id header).
3. **Verify the response.** Check `status`:
   - `"applied"` → `applied_commit` is the SHA on main. Done.
   - `"noop"` → nothing to merge. If this was unexpected, investigate
     (probably no commits on the branch, or already-applied).
   - `"hardening"` → recovery session running. Re-poll later, or wait for
     the `ChangeApplied` event.
   - `"conflict"` → focus `conflict_thread_id` to let CC resolve.
   - `"queued"` → the landing lane holds it. Wait for `ChangeApplied`.
4. **Emit a domain event.** When `status === "applied"`, emit
   `ProjectHardened` (or whatever domain event is appropriate) referencing
   `applied_commit` so downstream consumers can correlate.

The verification step used to require a separate `GET /api/v1/threads/:id`
call (or polling SSE) because the apply response was an empty `200 OK`.
That round-trip is gone — `applied_commit` in the response body is the
authoritative answer.

## Actor attribution

The handler resolves an actor from the request headers via
`api::actor::user_actor` and stamps it onto the emitted
`ChangeApplied` event. One header drives the agent-vs-human distinction:

- **`x-lucidos-agent-origin-token`**: the *thread-bound origin token*,
  shaped `<thread-id>@<depth>@<trigger>.<mac>`, minted per spawn under a
  per-engine-startup HMAC secret. A `-` stands in for any field the spawn
  does not carry. Set by the `lucidos` CLI from the
  `LUCIDOS_AGENT_ORIGIN_TOKEN` env var that the engine injects into every
  spawned subprocess.

The token is self-describing: when its MAC verifies, the spawning thread
is the prefix's first field, so nothing else on the request names it. The
actor stamps as `Api { mode: Agent, source_thread_id }`, which the UI
renders as **"Lucidos Agent"** with a popover linking back to the
spawning thread. Drop the header, or present a token that does not
verify, and the actor falls through to `Api { mode: Human, user_agent:
<UA> }`, which the UI renders as **"You"** (wrong attribution for an
agent-driven apply).

A second `x-lucidos-source-thread-id` header used to carry the thread id
separately. It was unverifiable, so any subprocess could claim any
thread; it is gone, and the engine reads no such header.

This is why the CLI exists: a `run_python` block calling
`urllib.request.urlopen(...)` ships with `User-Agent: Python-urllib/X.Y`
and no subprocess-origin headers, so the resulting `ChangeApplied`
misattributes to "You". The `lucidos changes apply <id>` subcommand
forwards the headers automatically; see
`system-knowhow/lucidos-cli.md` § `lucidos changes apply` for the
worked example.

## Common failure modes

| Symptom | Likely cause |
|---|---|
| `400` `{"error":"Change not found"}` | Wrong `change_id`, or change was discarded |
| `400` `{"error":"Change is already applied"}` | Stale UUID — the engine treats this as an error, not idempotent. The idempotent-applied path returns `200` with `status: "noop"`. |
| `403` `{"error":"Thread … is out of your reach, so you cannot apply a change from it…"}` | The caller presents a *thread-bound origin token* and the change belongs outside its own subtree. Applying it is the *workspace owner*'s, so the caller needs their *standing instruction*: a turn they opened, or a trigger firing they authorized. A caller with no token is unaffected. |
| `409` `{"error":"This change can't be applied in the thread's current state"}` | The owning thread doesn't currently offer Apply (e.g. mid-turn / Running). Mirrors the UI gate. Re-applying an *already-applied* change is exempt — that returns `200 noop`. |
| `200` `{"status":"noop", "commits_applied":0, ...}` | Branch existed but had no commits, or the change was already applied via another path |
| `200` `{"status":"hardening", ...}` | The change wasn't hardened. The recovery session will auto-apply when done. |
| `200` `{"status":"conflict", "conflict_thread_id":"..."}` | The merge needs human/agent intervention. Focus the thread. |
| `200` `{"status":"queued", ...}` | The landing lane holds the change behind or inside a resolving lane batch. |
| `200` `{"status":"applied", "applied_commit":null, ...}` | External-repo handoff (no main merge happens). The branch is kept in the external repo for the user to push/PR. |

## Safety context

The apply endpoint refuses to silently apply a change with declared files
when the branch ref has no commits — see `recover_no_commits_branch()` in
`crates/lucidos-engine/src/engine/git_ops.rs`, called from
`crates/lucidos-engine/src/engine/change_ops.rs` around the
`!has_branch_commits(...)` check. Before that fix, an
empty branch could mark a change "applied" while uncommitted CC work in
the worktree was silently discarded.

The new response shape is the next layer of the same defense: even after
the engine refuses to silently apply, the *response* must not silently
succeed. A `200 OK` with no body cannot be distinguished from a healthy
apply, so the assistant or any other caller might wrongly conclude work
landed. The `status` field forces an explicit declaration of what
happened.

## Related

- Event: `ChangeApplied` — emitted into the owning thread when the merge
  completes. Payload includes `commits` (subjects, oldest first) and
  `requires_restart`. Defined in
  `crates/lucidos-engine/src/engine/thread_events.rs`.
- Engine method: `LucidosEngine::apply_change` in
  `crates/lucidos-engine/src/engine/change_ops.rs` — the underlying logic
  that the HTTP handler delegates to. Returns `ApplyResult` (defined in
  `crates/lucidos-engine/src/engine/types.rs`).
- TypeScript client: `applyChange()` in
  `crates/lucidos-app/src/api/client.ts` — typed wrapper. The
  `ApplyChangeResult` interface mirrors the JSON shape above.
- CLI subcommand: `lucidos changes apply <id>` in
  `crates/lucidos-cli/src/changes.rs` — canonical caller for any
  subprocess context. Auto-forwards the actor-attribution headers via
  `crate::http::client()`; see `system-knowhow/lucidos-cli.md` for the
  user-facing reference.
