---
name: Coding Agent Lifecycle Events
description: What ThreadEvents the engine emits during a Claude Code / Codex coding-agent session, and which can wire a trigger today. Load when a workspace wants to "notify me when Claude Code finishes / asks a question / errors", "trigger on a CC session event", "wait until the agent is idle", "watch for a permission prompt", or talks about "claude code", "CC session", "coding agent", "agent idle", "session waiting on me", "permission request", "AskUserQuestion". Documents the full payload of each event, calls out the high-volume streaming events not to trigger on, the question-vs-permission unification, and the gap that no `CodingAgentErrored` exists today.
---

# Coding Agent Lifecycle Events

`CodingAgent*` is the engine's umbrella name for events emitted by a coding-agent session — Claude Code or Codex. Both backends translate their CLI output into the same `ThreadEvent` enum (the variants carry an `agent: AgentKind` field that distinguishes them; default is `ClaudeCode` for legacy DB rows).

This file enumerates the full set, marks which are safe to subscribe a trigger to, and points out two recurring sources of confusion: there is no `CodingAgentPermission*Event` separate from `UserQuestionAsked`, and there is no `CodingAgentErrored` event at all.

For the **full ThreadEvent enum** (chat-side, lifecycle, changes, background bash, plugin / repo, transient SSE-only commands — everything outside the CC + `UserQuestion*` slice this file covers), see `system-knowhow/thread-events.md`. That file also documents the persistence + allowlist status for every variant in one place; this file is the CC deep-dive.

For trigger config syntax (cron vs `on_event`, `condition` filters, `run.intent` discipline), see `system-knowhow/building-a-trigger.md`. For event-store column shape and the chat-side terminator events (`ResponseGenerated` / `ResponseFailed` / …), see `.claude/rules/db.md`.

## CRITICAL: today only `UserQuestionAsked` actually fires triggers

The scheduler subscribes to the EventBus and forwards events to the trigger matcher, but the `BusEvent::Thread` branch is gated by an explicit allowlist (`crates/lucidos-engine/src/scheduler/mod.rs`, look for `// Allow a curated subset of ThreadEvents`). Today that allowlist contains exactly one entry: `UserQuestionAsked`.

That means right now:

- `on_event: UserQuestionAsked` — works.
- `on_event: CodingAgentIdled` — **does not fire today**. The trigger config will validate and the trigger row will be persisted, but the matcher will never see the event because the scheduler skips it.
- `on_event: CodingAgentToolCalled` / `CodingAgentTextStreamed` / etc. — same, won't fire (and you wouldn't want them to — see Volume Class below).
- `on_event: <any chat-side event>` (`ResponseGenerated`, `ResponseFailed`, …) — also won't fire from a thread; the matcher only sees `SystemEvent::DomainEvent` (workspace-emitted via `emit_event`) and the allowlisted `ThreadEvent` slice.

If a workspace asks for "notify me when CC finishes" and you reach for `on_event: CodingAgentIdled`, **first** add `CodingAgentIdled` to the scheduler allowlist (engine code change, not a workspace config), or arrange for an existing path to emit a domain event the trigger can listen to. Don't ship a trigger that silently never fires.

## The full enumerated list

All variants below are defined on `ThreadEvent` in `crates/lucidos-engine/src/engine/thread_events.rs`. Each has a `#[serde(alias = "ClaudeCode<X>")]` for the legacy pre-rename name — write new code (and new triggers) against the `CodingAgent*` form.

### Persisted, low-volume — terminal-state or one-per-turn

| Event | When it fires | Volume |
|---|---|---|
| `CodingAgentUserMessageSent` | A user message was relayed into the agent's input stream. One per user-typed message on a CC thread. | One per user message |
| `CodingAgentPromptSent` | An engine-synthesized prompt was injected (orphan-recovery, hardening retrigger, merge-conflict explainer, post-question continuation). Carries an `origin: Option<MessageOrigin>` so the route popover can render "Engine · …". Persisted for audit; not rendered as a chat bubble. | One per engine-driven injection |
| `CodingAgentSettingsChanged` | User changed model, reasoning effort, or permission mode mid-session via the in-thread control. Persisted so the change survives idle exit + respawn. | Rare — only on user toggle |
| `CodingAgentPermissionRequest` | CC's MCP permission-prompt subprocess asked to confirm a tool call (Edit/Write/Bash on a path outside cwd, anything under `.claude/` or `.git/`). The user resolves it via `POST /api/v1/permission/<request_id>/{allow,deny}`. | Per tool call needing consent |
| `CodingAgentPermissionResolved` | The above request was answered (or auto-resolved by recovery). Carries `allowed: bool`, an optional `persist_scope` (`narrow`/`broad`/`session`) recording which "Always allow"-style scope the user picked, and a `reason` for failure / orphan-recovery cases. | Pairs 1:1 with `CodingAgentPermissionRequest` (orphan-recovery emits a `Resolved` with `reason: "Coding agent terminated before answering — request expired"` if the agent died first) |
| `CodingAgentIdled` | **The turn-boundary marker.** Emitted at the end of every CC turn whose Result wasn't an engine-shutdown abort. See "`CodingAgentIdled` semantics" below for the full payload. | One per turn (a session normally has 1–3 across its life; many more if the user keeps replying) |
| `MissingHardeningDetected` | Engine detected that a CC session ended without running the required `/harden` and auto-spawned a recovery hardening session. **Not a session terminator** — the thread stays active until hardening finishes. | Rare; only on the recovery path |

### Persisted, **HIGH-VOLUME** — DO NOT TRIGGER ON

These fire many times per turn — one event per streamed text chunk, per tool call, per tool result. A trigger on any of them would fire hundreds of times in a normal CC turn and would saturate whatever it's wired to. Subscribe a trigger to these only if you really mean it (and even then, almost certainly via a `condition` filter on the payload — and even then, see the allowlist note above before assuming the trigger will fire at all).

| Event | When it fires |
|---|---|
| `CodingAgentTextStreamed` | Each `text` chunk CC streams to the user. One per assistant-message line / paragraph as CC writes it. |
| `CodingAgentToolCalled` | Each tool invocation CC makes. Carries `name`, `args` (full JSON), optional `description`, and `tool_use_id` so the matching `ToolResult` can be paired even when a permission prompt splits them across exchanges. |
| `CodingAgentToolResult` | The result returned to CC for a prior `ToolCalled`. Carries the same `tool_use_id`. |

### Transient — never persisted, broadcast over SSE only

| Event | When it fires |
|---|---|
| `CodingAgentThreadSpawned` | A child CC thread (spawned via `run_claude` / `run_thread`) has started — carries the new `cc_thread_id` + `title`. SSE-only: the persisted record of the child is its own thread row. |

### `UserQuestion*` — the question / permission channel

These are NOT prefixed `CodingAgent*` because the same machinery serves any agent that needs to ask the user a structured question (today: CC's built-in `AskUserQuestion` tool, and the MCP permission prompt — see "Question vs. permission" below).

| Event | When it fires |
|---|---|
| `UserQuestionAsked` | An interactive question has been raised — typically by CC's `AskUserQuestion` tool or by a permission-prompt path that routes through the same registry. The CC subprocess is killed at intercept; resume happens via `POST /api/cc/answer-question` (which then emits `UserQuestionAnswered` and respawns CC with `--resume`). |
| `UserQuestionAnswered` | The user (or, on the orphan-recovery path, the engine) supplied an answer. Pairs 1:1 with the matching `UserQuestionAsked` via `tool_use_id`. |

## Concrete payload shapes

### `CodingAgentIdled`

```json
{
  "type": "CodingAgentIdled",
  "data": {
    "has_changes": true,
    "is_external_repo": false,
    "requires_restart": false,
    "cc_session_id": "abc123-…",
    "agent": "ClaudeCode",
    "worktree_path": "/Users/.../.lucidos/worktrees/thread-1a2b3c4d",
    "worktree_head_sha": "f6ae7364e…"
  }
}
```

All fields except `agent` are `#[serde(skip_serializing_if = ...)]`-gated and will be missing from the wire when at their zero value (`false` for bools, `None` for `Option`s). Read defensively: `payload.has_changes ?? false`, `payload.worktree_path ?? null`.

| Field | Type | When present |
|---|---|---|
| `has_changes` | `bool` | `true` iff the CC branch has a non-empty net diff against the repo's main branch, **after** filtering out runtime-only paths (`.lucidos/**` etc. — see `branch_changed_files` + `files_require_restart`). Carries forward from a prior idle if the live turn produced no new commits but the branch still has prior work. |
| `is_external_repo` | `bool` | `true` for sessions running against a repo imported via `RepositoryImported` (i.e. not the Lucidos engine repo itself). |
| `requires_restart` | `bool` | Derived from the same filtered file list — `true` iff at least one changed file matches `files_require_restart` (Rust source, `Cargo.lock`, certain bundled assets). |
| `cc_session_id` | `Option<String>` | The CC CLI session id at the moment of idle. `None` for recovery-emitted idles where no live subprocess existed (see "no-branch" and "stuck-session" recovery paths). |
| `agent` | `AgentKind` | `"ClaudeCode"` or `"Codex"`. Defaults to `"ClaudeCode"` on legacy DB rows. |
| `reason` | `Option<String>` | **Usually absent.** Stamped only by recovery: `"engine_restart_interrupt"` when a mid-turn-crashed session is surfaced to the UI as "interrupted, click to continue" instead of being auto-spawned. The frontend reads this to render the continue affordance. |
| `worktree_path` | `Option<String>` | Absolute filesystem path of the worktree the agent ran in. Populated by `run_session.rs` for normal turns. **`None`** when the worktree was `worktree remove --force`'d before the idle fired (the "stale session" cleanup path, the no-branch recovery path) and on legacy rows. |
| `worktree_head_sha` | `Option<String>` | Snapshot of `git rev-parse HEAD` in the worktree at idle time, used by the next spawn to detect external user edits made between turns. `None` on legacy rows, when no worktree, or when `git rev-parse` fails (e.g. zero-commit branch). |

**Fires on:** every CC turn that ended on a `Result` other than engine-shutdown abort. That covers natural completion, `Failed` (CC errored, OOM, empty assistant text), and user `Cancel` (the `Stop` button — cancel is treated as a turn boundary, not a terminator). Engine shutdown does NOT emit `CodingAgentIdled` — recovery resumes the session on next start.

### `UserQuestionAsked`

```json
{
  "type": "UserQuestionAsked",
  "data": {
    "tool_use_id": "toolu_…",
    "cc_session_id": "abc123-…",
    "question": "Which approach should I take?",
    "options": [
      { "id": "opt-0", "label": "Approach A", "description": "…" },
      { "id": "opt-1", "label": "Approach B" }
    ],
    "worktree_path": "/Users/.../worktrees/thread-…",
    "multi_select": false
  }
}
```

| Field | Type | Notes |
|---|---|---|
| `tool_use_id` | `String` | CC's identifier for this question; used as the unique key in DB and in `UserQuestionAnswered.tool_use_id`. |
| `cc_session_id` | `String` | The CC session id at the moment of intercept. Resume pins to this id via `--resume`. |
| `question` | `String` | The prompt text shown to the user. **For permission requests, this is the human-readable rendering of the tool-use that CC asked permission for** — see "Question vs. permission" below. |
| `options` | `Vec<QuestionOption>` (default `[]`) | Each option is `{ id, label, description? }`. Empty for free-text-only prompts. |
| `worktree_path` | `Option<String>` | The CC worktree at intercept. Required for `--resume` to find the session JSONL (CC keys session storage by CWD). `None` only when the request came in without a worktree context. |
| `multi_select` | `bool` (default `false`) | `true` when the user may pick multiple options. |

### `UserQuestionAnswered`

```json
{
  "type": "UserQuestionAnswered",
  "data": {
    "tool_use_id": "toolu_…",
    "answer": { "kind": "Selected", "option_id": "opt-0" }
  }
}
```

`answer` is a tagged union (`AnswerKind`):

- `{ "kind": "Selected", "option_id": "..." }`
- `{ "kind": "FreeText", "text": "..." }`
- `{ "kind": "MultiSelected", "option_ids": [...], "text": "..."? }`
- `{ "kind": "Canceled" }` — user closed the question without picking

**Pairing guarantee.** A `UserQuestionAnswered` is emitted exactly once per `UserQuestionAsked` it answers, enforced by a unique DB index on `(thread_id, tool_use_id)`. The pair can be left unbalanced in real life:

- The agent process dies before the user answers, the engine restarts, and the question is never revisited from CC's side. The `Asked` row stays in the DB without a matching `Answered`. There is **no engine-side timeout that auto-emits `Answered`** for the AskUserQuestion path. (The MCP permission path, `CodingAgentPermissionRequest`, does have an orphan-recovery sweep that emits a `CodingAgentPermissionResolved { allowed: false, reason: "Coding agent terminated before answering — request expired" }` — but that's a different event family.)
- The user closes the popover via the cancel control — that fires `UserQuestionAnswered { answer: { kind: "Canceled" } }`, which still counts as a pair.

So a workspace listening for `UserQuestionAsked → UserQuestionAnswered` should treat a long-pending `Asked` with no matching `Answered` as "the user is still being prompted" rather than as a guaranteed-future event.

### `CodingAgentPermissionRequest` / `CodingAgentPermissionResolved`

Used by the MCP permission-prompt subprocess (the `mcp__permission-prompt` tool family). Distinct from `UserQuestionAsked`/`UserQuestionAnswered` — see the next section for why both exist.

```json
{
  "type": "CodingAgentPermissionRequest",
  "data": {
    "request_id": "req_…",
    "tool_use_id": "toolu_…",
    "tool_name": "Edit",
    "input": { "file_path": "...", "old_string": "...", "new_string": "..." },
    "summary": "Edit /path/to/file.rs"
  }
}
```

```json
{
  "type": "CodingAgentPermissionResolved",
  "data": {
    "request_id": "req_…",
    "allowed": true,
    "reason": null,
    "persist_scope": "session"
  }
}
```

`persist_scope` is `"narrow"` / `"broad"` / `"session"` when the user clicked "Always allow"-style; `null` for plain Allow-once / Deny / orphan-recovery. The frontend reads it to render the answered card with a check on the chosen button and strike-through on the rest.

## Question vs. permission — and why both event families exist

There are two distinct user-facing prompt mechanisms in play:

1. **CC's native `AskUserQuestion` tool.** A first-class tool CC can call to get structured input mid-turn ("which approach", "is this OK to do"). Fires `UserQuestionAsked` / `UserQuestionAnswered`. The `question` payload field is the literal prompt text CC supplied.
2. **MCP permission prompts.** A separate subprocess CC consults whenever it would invoke `Edit`/`Write`/`Bash` on a path it doesn't have a static allow rule for. Fires `CodingAgentPermissionRequest` / `CodingAgentPermissionResolved`. The user is choosing whether to authorize a specific tool call, possibly with persistence ("Always allow Bash(git:*)").

These look similar in the UI (both render as a card the user has to act on) but they are two separate event lanes. **A "permission prompt" is not a third event family**; it's whichever of these two raised it.

In practice today:

- A workspace LLM that says "notify me when Claude Code is asking for permission" almost always means *either*: it should subscribe to `UserQuestionAsked` (covers the AskUserQuestion case and any permission-style prompt that the engine routes through the question registry), *or* — for true MCP permission prompts — `CodingAgentPermissionRequest`. Pick based on what the user actually wants to act on.
- The seeded "session waiting on me" push-notification trigger in stock Lucidos installs is wired to `on_event: UserQuestionAsked`. That's the pattern to copy for "wake me when CC needs me."
- `CodingAgentPermissionRequest` is **not** in the scheduler's ThreadEvent allowlist either, so a trigger on it won't fire today without an engine code change. Same caveat as `CodingAgentIdled`.

## The error gap

There is no `CodingAgentErrored`, `CodingAgentFailed`, or `CodingAgentCrashed` event. Verified via `rg -n "CodingAgent(Error|Errored|Failed|Crashed|Aborted|Canceled)" crates/lucidos-engine/src/` — zero matches.

When a CC session fails mid-turn (upstream API error, OOM-killed bash, empty assistant text on a non-cancel turn), the engine routes it through `classify_result` (`agent_session/lifecycle.rs`) and:

- emits a chat-side `ResponseFailed { error }` to mark the turn as failed;
- emits `CodingAgentIdled { has_changes: <whatever was on the branch> }` so the dispatcher closes the turn and the UI exits the "Working" state.

`ResponseFailed` is **not in the trigger ThreadEvent allowlist** (which only contains `UserQuestionAsked` — see the top of this file). And the scheduler's auto-error notification mechanism — the one that posts a notification when a trigger run blows up — is scoped to *trigger* failures specifically, not to coding-agent failures inside a thread. A workspace that asks for "notify me when Claude Code errors" cannot be wired today without first picking one of:

1. Add a new `CodingAgent*` failure variant at the `classify_result` site, persist it, and add it to the scheduler ThreadEvent allowlist.
2. Add `ResponseFailed` (and/or `CodingAgentIdled`) to the scheduler ThreadEvent allowlist, and use a `condition` payload filter to scope to the CC case.
3. Have whoever notices the failure (the agent itself, an external watcher) `lucidos events emit CodingAgentFailureObserved {...}` — domain events DO go through the trigger matcher.

Tell the user it's not a one-line trigger config before you start writing one.

## Notes on `CodingAgentPromptSent`

This is NOT the user typing into CC. User-typed input fires `CodingAgentUserMessageSent`. `CodingAgentPromptSent` is for prompts the engine itself synthesized:

- merge-conflict explanation injected after `MergeConflictDetected`,
- `MissingHardeningDetected` recovery prompt asking CC to run `/harden`,
- the empty `CodingAgentPromptSent` emitted right after `UserQuestionAnswered` so the timeline shows a "thinking" placeholder while CC processes the answer.

Distinguishable on the wire by `origin: Some(MessageOrigin::Engine { reason: ... })`. Workspaces should generally not need to subscribe to this — it's an audit trail event, not a lifecycle signal.

## Recipe-shaped guidance

For the trigger config field reference (cron format, `on_event`, `condition` operators), see `system-knowhow/building-a-trigger.md`. The condition language is `$eq` / `$ne` / `$lt` / `$lte` / `$gt` / `$gte` / `$in` over top-level payload fields (a bare value is `$eq`); see `crates/lucidos-engine/src/triggers/condition.rs` for the full operator set.

### Notify when CC is waiting on the user

```yaml
on_event: UserQuestionAsked
run:
  intent: "Send me a push notification that Claude Code is waiting on my answer."
```

This is the only CC-lifecycle trigger that **works today out of the box**. Stock installs ship with exactly this trigger seeded.

To scope to a specific CC session, add a `condition`:

```yaml
on_event: UserQuestionAsked
condition:
  cc_session_id: "abc123-…"
```

Conditions are pure payload filters — they only look at the event's payload fields. `cc_session_id` is on `UserQuestionAsked`'s payload (see shape above), so this works. There is no conditional access to thread metadata, app id, etc. from the condition.

### Notify when a CC session finishes / produced changes

The natural answer is `on_event: CodingAgentIdled` with `condition: { has_changes: true }`, but **this does not fire today** — `CodingAgentIdled` is not in the scheduler's ThreadEvent allowlist (see top of file). Tell the user up front: this needs the allowlist expanded in `crates/lucidos-engine/src/scheduler/mod.rs` first. Once that's done, the config would look like:

```yaml
on_event: CodingAgentIdled
condition:
  has_changes: true
run:
  intent: "Tell me Claude Code finished and left a change to review."
```

(Adding `is_external_repo: { $ne: true }` would scope to the engine repo, etc. — same condition language as above.)

### Notify when CC errors

Not directly wireable today (see "The error gap" above). The honest answer to a user asking for this is: "We need to either add a `CodingAgent*` failure event to the engine, or have you emit a domain event from wherever you notice the failure, before a trigger can subscribe to it."
