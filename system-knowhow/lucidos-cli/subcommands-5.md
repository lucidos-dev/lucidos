---
name: Lucidos CLI (lucidos), Subcommands (continued)
description: Part of Lucidos CLI (lucidos). Its index lists the sections it covers.
---

One part of `system-knowhow/lucidos-cli`, which lists every part.

## Subcommands (continued)

### `lucidos snapshot`

Local workspace snapshots capture the database and backup-scope workspace files
under an opaque snapshot handle. Keys remain in the engine's key store. Chat
agents and scripts both use this CLI family; workflow and safety rules live in
`system-knowhow/backups/workspace-snapshots`. Every command
accepts `--workspace <name-or-absolute-path>` to target a workspace and `--json`
for compact JSON; otherwise it prints formatted JSON. Every caller must identify
itself through the ordinary device or local-process credentials.

Create/check share an archive slot with backups. Busy returns exit 75; a refused
HTTP request returns 2; a failed job or server operation returns 4. Successful
requests return 0. Restore commands are described below.

`restore` takes an exact `--workspace` gateway slug, a matching
`--expected-workspace-id`, the handle, and an optional `--reason`. It refuses
the calling agent's own workspace. The target
checks the source through the gateway and enforces its restore policy and any
source-thread permission. Never retry a lost restore response.

`restore-status` takes the restore id and `--workspace`, and reads the gateway's
journal projection. Its subsection below has the details.

#### `lucidos snapshot create`

`lucidos snapshot create [--label <text>] [--keep] [--wait] [--json]`
starts a durable capture and returns `{job_id, handle}`. `--wait` polls until the
job completes and prints the job with its safe manifest or typed failure.
`--keep` protects the snapshot from retention. Files may be newer than the
database capture; the manifest records the live capture window.

#### `lucidos snapshot list`

`lucidos snapshot list [--json]` lists authenticated safe manifests: handles,
labels, kinds, timestamps, component sizes and kept flags. Capture/list/get/check
use the archive slot, so a list can be busy during a capture.

#### `lucidos snapshot get`

`lucidos snapshot get <handle> [--json]` reads one authenticated safe manifest.
It does not verify every archived byte; use check for that.

#### `lucidos snapshot check`

`lucidos snapshot check <handle> [--json]` starts verification and waits for the
result. It checks the manifest MAC, component hashes, dump contents and file
index, and restores nothing.

#### `lucidos snapshot status`

`lucidos snapshot status <job-id> [--wait] [--json]` reads a durable capture or
check job. Status needs no archive slot or snapshot key, and remains readable
during captures. A lost CLI connection does not cancel the job; run status again
with the returned job id. `--wait` polls to a terminal result.

#### `lucidos snapshot restore`

`lucidos snapshot restore <handle> --workspace <gateway-slug>
--expected-workspace-id <same-slug> [--reason <text>] [--port <gateway-port>]
[--wait] [--json]` requests restore of another workspace as the issuing agent. It requires
the thread-bound origin token. The issuing engine checks that token over its
HTTP API and names the source workspace, never a directory basename. The
gateway admits the restore calls on that token alone, so the command works
inside the agent sandbox and reads no local token.

It refuses its
own workspace before probing the gateway, and refuses ambiguous gateway
discovery. The target enforces registry binding and agent policy. The gateway
refuses a target that is not a *machine-made workspace* with
`snapshot_restore_target_protected` (ADR 0671). A machine-made target is
yours, so the restore needs no card: your thread records it as
`MachineMadeWorkspaceChanged` with the snapshot handle (ADR 0676). Under the
target's `allow` policy your workspace is not asked, so only the target
records the restore. A terminal
without an issuing agent must use the owner UI.

The restore POST is sent once, without redirects or retries, including after
refusal or a lost reply. A lost reply reports delivery unknown: inspect restore
status rather than retrying admission or spending approval again. Conflict
returns exit code 75; other client refusals return 2, and server failures return
4. The JSON response preserves target refusal details and any restore identity.

`--wait` polls that exact restore journal through the gateway until completion
or terminal failure. It waits past the offline file swap for boot verification.
Success returns 0; rollback, refusal, unchanged failure or needs-attention
returns 4. The wait performs only status GETs and never resubmits admission.
If a status read fails, use `restore-status` with the admitted restore id printed
on stderr; do not request another restore.

#### `lucidos snapshot restore-status`

`lucidos snapshot restore-status <restore-id> --workspace <gateway-slug>
[--port <gateway-port>] [--json]` reads the gateway's independent restore journal
projection, including while the target engine is stopped. It refuses ambiguous
gateway discovery and mismatched journal or workspace identities. The CLI asks
for that exact retained journal, including older completed restores; an absent
requested journal is reported unavailable rather than replaced with another.
Terminal failure or needs-attention phases return exit code 4. This read never
starts or retries a restore.

#### `lucidos snapshot delete`

`lucidos snapshot delete <handle> [--json]` deletes a snapshot and announces it.
Recovery snapshots require a device actor; a local subprocess cannot delete
them. Open restore references and the newest recovery snapshot are protected.

After successful captures, `snapshot_retention` keeps 5 unkept manual snapshots
by default and `snapshot_recovery_retention` keeps 3 recovery snapshots. Kept
snapshots and open restore references remain protected. Set these global
preferences through the ordinary preferences surface.

### `lucidos model-prices`

Prints the *price table* as JSON: the reply of `GET /api/v1/model-prices`. That is the cards Lucidos ships, merged card by card with the workspace override in `data/config/model-prices.json`. Each card carries its `origin` (`shipped` or `override`), and `override.error` says why an override was ignored. The shipped cards live inside the engine, so a script reads prices here rather than from a file. The price check writes the override, as `model-price-check.md` describes.

```bash
# What does Opus 5.5 charge today, and where does that card come from?
lucidos model-prices | jq '.models["claude-opus-5-5"][-1]'
```

### `lucidos ask-permission <action> --thread <uuid> --reason <R>`

Ask the *workspace owner* for a *permission* to do one act outside your own authority: an act outside your own subtree, or one saved draft's send (ADR 0688). Use it when a clause-4 act was refused, or will be, and the owner has not opened your current turn. Examples: applying another thread's change, or archiving a thread outside your subtree.

It takes two steps, because a question card lives inside the turn that asks it:

1. Run `lucidos ask-permission apply --thread <uuid> --reason "The release needs the e2e fix first."`. It records a *permission request* on your own thread and prints an id like `permission-request:<uuid>`.
2. Call `AskUserQuestion` (Codex: `ask_user_question`) with exactly one question whose text is that id, and any two options. Lucidos shows its own card in that question's place: it names the act, offers the scopes the action allows, and quotes your reason as yours.

The card's options come from the action's menu. Every card also offers **Don't allow**.

| Action | Allow once | Allow for this thread | Always allow |
|---|---|---|---|
| `archive`, `cancel`, `continue`, `control`, `set-aside`, `bring-back`, `ask-side-question`, `dismiss-side-question` | that thread | that thread | that thread |
| `apply`, `revert` | that thread | that thread | no |
| `discard`, `submit-draft` | that exact target | no | no |
| `edit-intelligence-ladder` (no `--thread`) | one ladder edit | every ladder edit | no |

If the owner allows it, do the act next, in this turn. **Allow once** covers one matching call, and ends when your thread's next turn starts. An engine resume and an event-wait wake do not count as one. **Allow for this thread** lasts as long as your thread. **Always allow** holds for every thread until the owner revokes it (§ "`lucidos permissions`").

- **Actions**, each with `--thread <uuid>`: the ones in the table. `submit-draft` needs the owner even for a draft inside your own subtree (§ "`lucidos threads submit-draft`"). The workspace actions take `--workspace <slug>` instead (§ "The broker verbs"). A handoff to a *successor session* needs no permission, and `spawn-successor` is retired. `answer-question` and `resolve-permission` are refused: a card never asks to answer another card.
- **No permission for a spawn.** Creating a top-thread and moving your own child to top level need none (ADR 0475). `create-top-thread` and `detach-child` answer a 409 that says so.
- **Refusals**: a 409 when you already hold the authority: your turn carries the standing instruction, the target is inside your own subtree for any action but `submit-draft`, or a live permission covers it. Act at once then. A 403 outside a coding-agent session. A 400 for a missing reason or a missing `--thread`.
- **Only the owner can allow it.** An agent answering the card is refused, including the agent that asked. The owner taps an option. A typed reply answers the card as free text and allows nothing, as do "Don't allow", Cancel and no answer.

Over `POST /api/v1/permission-requests`, with body `{action, target_thread_id, reason}`. The request lands on the thread your origin token names. `lucidos ask-owner-approval` is the retired spelling, kept as a hidden alias for one release.

### `lucidos ask-permission <action> --trigger <id> --reason <R>`

Use `grant-trigger-programs` with repeatable `--program <name>` or
`grant-trigger-folders` with repeatable `--folder <path>` to ask before a run.
Use a program name without arguments, or `Python` for the Python tool.
Use existing absolute folder paths without `..`. The root and home directories
are refused. Without these flags, Lucidos fills the card from recorded program
calls or folder refusals. The request body carries `action`, `target_trigger_id`,
`reason` and the optional `programs` or `folders` array.

Ask your question tool with the printed request ID as above, and wait for the
answer before asking about another trigger. The card offers **Always allow**,
**Always deny** and **Cancel**, never Allow once: no run waits on it, so a grant
has nothing to cover but future runs. The answer performs the change directly.

- **Always allow** saves the grants shown on the card in that trigger's config.
- **Always deny** saves them as *trigger denies*, so later runs that need them
  end without a card.
- **Cancel** saves nothing.

A blocked intent run's own card still offers Allow once for that one run
(`system-knowhow/triggers/grouping-triggers.md` § Program grants and § Trigger denies).

### `lucidos await-event --on <EventType> [--on <EventType> ...] [--condition <JSON>] --timeout-secs <N> --reason <R>`

Subscribe the **calling thread** to a Lucidos event, then finish. The engine
re-opens the thread with a follow-up message when a matching event lands, or
tells it the deadline passed. It is the coding-agent counterpart of the chat
agent's `await_event` tool, on the same registration, so both get the same caps
and refusals.

**It returns immediately and blocks nothing.** The thread is plain **idle** while
it holds a subscription: no queue slot, no running turn, nothing for the user to
resolve. So the shape is *subscribe, say what you are waiting for, end the
session*. It replaces a sleep-and-recheck loop. Polling for the event as well is
worse than either.

Use it whenever you wait on something the engine emits: a change appearing
(`ChangeProposed`), a trigger firing (`TriggerExecuted`), a backup finishing
(`BackupCompleted` / `BackupFailed`), or a domain event your own scripts emit.
Any persisted event works. A transient frame such as `BackupProgress` is refused
by name. It is **not** for external state with no Lucidos event (a third-party
API, a file another process may write): nothing would be delivered, so poll.

**And never subscribe to your own child's completion.** A thread spawned with
`lucidos spawn-thread --relation child` reports to this one on its own. When it
fails under any mode but `no-wake`, or its *wake mode* wakes you, the engine emits `ChildThreadCompleted`
here. It re-opens this thread with the child's status, summary,
`pending_change_ids` and `sub_thread_pending_changes`. A report its mode
files lands as a `ChildReportFiled`, which re-opens nothing: you read it at the
front of your next prompt. So when you need the result to continue, pass
`--wake-mode wake`.

A wait on your own child buys nothing and costs two things:
one of the subscriptions the loop cap allows, and a second clock. A child that
outlives `--timeout-secs` re-opens this thread with a pointless expiry, then
again when it finishes.

Await a `ChildThreadCompleted`, with `--on ChildReportFiled` beside it, only for
a completion that is **not** your own child's, named with
`--condition '{"child_thread_id": "<uuid>"}'`. Matching is
workspace-wide, so that is any thread's child, not only a descendant of yours.
A coding-agent session another thread spawned is a first-class thing to watch.
A session **nobody** spawned (the user started it) has no
`ChildThreadCompleted`, since only the parent/child fan-in emits one. Watch its
turn boundary instead: `--on CodingAgentIdled --condition '{"thread_id": "<uuid>"}'`.
That fires on every idle, a user Stop included, so it means "the agent went
idle", not "the work is done".

A **rendezvous, not a stream**. The first match resolves the subscription and
consumes it. "Continue when the next X happens" is this. "React to every X,
forever" is a *trigger*.

**It watches forward only, so still check whether it already happened.** A
subscription cannot fire for an event already gone by, so look at state first.
The race between that check and this command is covered: if a match landed in
the few minutes before it, the response names it, with its age. Read that part,
not just the `"status":"subscribed"`, and act on it before you finish, because
nothing will deliver it. Only you can tell an event you missed from one you
handled a few minutes ago.

- `--on` names the event type, PascalCase past tense. Repeat it to watch
  several: any one of them re-opens the thread.
- `--condition` is a JSON object filtering the event's OWN payload by field
  path (a dot reads one level down), applied to every `--on` name. Equality by
  default, or an operator object: `{"$eq":v}`, `{"$ne":v}`, `{"$lt":n}`,
  `{"$lte":n}`, `{"$gt":n}`, `{"$gte":n}`, `{"$in":[…]}`, `{"$nin":[…]}`,
  `{"$regex":"…"}`. `$or` in key position takes a list of whole conditions.
  See `system-knowhow/triggers/writing-cron-expressions.md` § "What a condition can say" for the full
  language.
- A thread event always offers one field beyond the payload: `thread_id`, from
  the thread the event belongs to. So `--condition '{"thread_id":"<uuid>"}'`
  scopes the wait to one thread. It does not appear in `lucidos events query`
  output (the row holds the thread in its own column). A **domain event**
  belongs to no thread, so it has none.
- `--timeout-secs` is required and capped at 86400 (24 h). There is no unbounded
  subscription. Giving up early costs one turn. Giving up too late costs the
  user the whole wait.
- `--reason` is a short noun phrase in the user's language, naming **what** you
  await. They read it in the waiting indicator, and it tells a sleeping thread
  from a stalled one. Write `"the build slot to free up"`, not
  `"waiting for the build slot"`: the transcript labels it `Waiting for <reason>`,
  so a waiting word says it twice.

Refusals arrive as a `400` with the reason. Read it rather than retrying:

- A per-token streaming event (`TextStreamed` and friends) or an `EventWait*`
  type is refused outright.
- A thread may hold at most 25 live subscriptions.
- The same `--on` list twice on one thread is refused (it would deliver one
  event to you twice).
- The loop cap: 20 counted subscriptions within an hour with no message or
  answer from the user. A subscription another thread's event ended does not
  count.

```bash
# Wait for a domain event the workspace's own scripts emit, then stop. The
# engine re-opens this thread with the payload when it lands.
$ lucidos await-event --on E2ETestsPassed --timeout-secs 3600 \
    --reason "tonight's e2e run to report"

# Narrow it: only a change that actually touched files.
$ lucidos await-event --on ChangeProposed --condition '{"file_count": {"$gt": 0}}' \
    --timeout-secs 1800 --reason "the refactor to propose its change"
```

