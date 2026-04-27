---
name: Apply Change API
description: How to apply CC-proposed changes from a thread worktree to main, verify success, and emit the ChangeApplied event
---

# Apply Change API

`POST /api/changes/:id/apply` merges a CC-proposed change branch into `main`.
The response is a typed JSON body that makes verification self-contained —
callers no longer need to poll thread state to confirm "did this land,
and at what SHA."

## Endpoint

```
POST /api/changes/{change_id}/apply
```

- Path: `change_id` — the UUID returned when CC proposed the change
  (`ChangeProposed` event payload, or `changes` table row).
- No request body.
- No query parameters.

## Response shape

Always JSON. `200 OK` on accepted requests (including `noop`, `hardening`,
`conflict`); `400 Bad Request` only when the engine could not even consider
the change (missing change, invalid status, etc.).

```ts
type ApplyStatus = 'applied' | 'noop' | 'hardening' | 'conflict';

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
| `conflict` | Merge conflict — a CC session is resolving it. `conflict_thread_id` points at the thread to focus. Includes the case where an in-place merge failed but the original session stays alive for retry. |

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
2. **POST** to `/api/changes/{change_id}/apply`.
3. **Verify the response.** Check `status`:
   - `"applied"` → `applied_commit` is the SHA on main. Done.
   - `"noop"` → nothing to merge. If this was unexpected, investigate
     (probably no commits on the branch, or already-applied).
   - `"hardening"` → recovery session running. Re-poll later, or wait for
     the `ChangeApplied` event.
   - `"conflict"` → focus `conflict_thread_id` to let CC resolve.
4. **Emit a domain event.** When `status === "applied"`, emit
   `ProjectHardened` (or whatever domain event is appropriate) referencing
   `applied_commit` so downstream consumers can correlate.

The verification step used to require a separate `GET /api/threads/:id`
call (or polling SSE) because the apply response was an empty `200 OK`.
That round-trip is gone — `applied_commit` in the response body is the
authoritative answer.

## Common failure modes

| Symptom | Likely cause |
|---|---|
| `400` `{"error":"Change not found"}` | Wrong `change_id`, or change was discarded |
| `400` `{"error":"Change is already applied"}` | Stale UUID — the engine treats this as an error, not idempotent. The idempotent-applied path returns `200` with `status: "noop"`. |
| `200` `{"status":"noop", "commits_applied":0, ...}` | Branch existed but had no commits, or the change was already applied via another path |
| `200` `{"status":"hardening", ...}` | The change wasn't hardened. The recovery session will auto-apply when done. |
| `200` `{"status":"conflict", "conflict_thread_id":"..."}` | The merge needs human/agent intervention. Focus the thread. |
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
