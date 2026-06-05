---
name: Coding Agent Lifecycle Events
description: What ThreadEvents the engine emits during a Claude Code / Codex coding-agent session, and which can wire a trigger. Load when a workspace wants to "notify me when Claude Code finishes / asks a question / errors", "trigger on a CC session event", "wait until the agent is idle", "watch for a permission prompt", or talks about "claude code", "CC session", "coding agent", "agent idle", "session waiting on me", "permission request", "AskUserQuestion". Documents the full payload of each event, the per-token streaming events the scheduler blocks, the question-vs-permission unification, and the gap that no `CodingAgentErrored` exists today.
---

# Coding Agent Lifecycle Events

`CodingAgent*` is the engine's umbrella name for events emitted by a coding-agent session — Claude Code or Codex. Both backends translate their CLI output into the same `ThreadEvent` enum (the variants carry a `coding_agent: CodingAgent` field that distinguishes them; default is `ClaudeCode` for legacy DB rows; the wire field has `#[serde(alias = "agent")]` so rows persisted before the rename still decode).

The session's *worktree* wraps the enclosing git, which differs by `coding_agent_kind`:

- `lucidos` (default) — full checkout of the Lucidos source repo.
- `app` — sparse-checkout of the workspace git narrowed to `data/apps/<id>/` (see *app worktree*). Apply ff-merges to the workspace git's local `main`; no engine restart; `/harden` does not run; `AppUiRefreshRequested` emits when iframe-bundled files change.
- `external` — full checkout of the user-registered external git repository. No Apply / Discard surface.

`coding_agent_kind` ships on every `SessionStarted` event and is persisted in `thread_summaries.coding_agent_kind` so the apply path can dispatch correctly without re-reading the event log.

This file enumerates the full set, marks which are safe to subscribe a trigger to, and points out two recurring sources of confusion: there is no `CodingAgentPermission*Event` separate from `UserQuestionAsked`, and there is no `CodingAgentErrored` event at all.

For the **full ThreadEvent enum** (chat-side, lifecycle, changes, background bash, plugin / repo, transient SSE-only commands — everything outside the CC + `UserQuestion*` slice this file covers), see `system-knowhow/thread-events.md`. That file also documents the persistence + triggerability status for every variant in one place; this file is the CC deep-dive.

For trigger config syntax (cron vs the `on` subscription list, per-entry `condition` filters, `run.intent` discipline), see `system-knowhow/building-a-trigger.md`. For event-store column shape and the chat-side terminator events (`ResponseGenerated` / `ResponseFailed` / …), see `.claude/rules/db.md`.

## Triggerability: blocklist semantics

The scheduler forwards persisted ThreadEvents to the trigger matcher unless they're in a small **per-token streaming blocklist** (`ThreadEvent::is_per_token_streaming` in `crates/lucidos-engine/src/engine/thread_events.rs`, gated at the `BusEvent::Thread` arm of the scheduler subscriber in `crates/lucidos-engine/src/scheduler/mod.rs`). The CC-relevant entry on the blocklist is `CodingAgentTextStreamed` — that's the only CC variant a workspace cannot subscribe to today.

That means right now (each line is one entry inside a trigger's `on` list — see `system-knowhow/building-a-trigger.md` for the full subscription shape):

- `event_type: UserQuestionAsked` — works. Wire this to a trigger that calls `send_notification` with `tap: { kind: 'navigate', to: { target: 'thread', id: '<thread_id>', event_id: '<source_event_id>' } }` (event_id taken from the trigger's `Source event id:` line) to push the user with a deep-link straight to the question.
- `event_type: CodingAgentIdled` — **works**. Pair with the entry's `condition: { has_changes: true }` to scope to "CC finished and left work to review."
- `event_type: CodingAgentPermissionRequest` — **works**. Lets a workspace react to "CC is asking permission for a tool call."
- `event_type: CodingAgentToolCalled` / `CodingAgentToolResult` / `CodingAgentPromptSent` — **works**, but these are per-action and chatty. Always pair with the entry's `condition:` (e.g. `name: "Bash"`, `args.command: { $regex: "git push" }`) — without one the trigger fires many times per turn.
- `event_type: CodingAgentTextStreamed` — does not fire; per-token streaming is the only thing the scheduler blocks. Subscribing to it is a no-op.
- `event_type: <any chat-side lifecycle event>` (`ResponseGenerated`, `ResponseFailed`, `ChangeApplied`, …) — works; same blocklist semantics. See `system-knowhow/thread-events.md` for the full set.

## The full enumerated list

All variants below are defined on `ThreadEvent` in `crates/lucidos-engine/src/engine/thread_events.rs`. Each has a `#[serde(alias = "ClaudeCode<X>")]` for the legacy pre-rename name — write new code (and new triggers) against the `CodingAgent*` form.

### Persisted, low-volume — terminal-state or one-per-turn

| Event | When it fires | Volume |
|---|---|---|
| `CodingAgentUserMessageSent` | A user message was relayed into the agent's input stream. One per user-typed message on a CC thread. | One per user message |
| `CodingAgentPromptSent` | An engine-synthesized prompt was injected (orphan-recovery, hardening retrigger, merge-conflict explainer, post-question continuation). Carries an `origin: Option<MessageOrigin>` so the route popover can render "Engine · …". Persisted for audit; not rendered as a chat bubble. | One per engine-driven injection |
| `CodingAgentSettingsChanged` | Two roles. (1) User changed model, reasoning effort, or permission mode mid-session via the in-thread control. (2) Emitted once at CC `Init` carrying `cc_session_id: Some(..)` — the moment CC reports its session id — so the id is durable in the event store *before* the first `CodingAgentIdled`. Persisted so settings survive idle exit + respawn, and so a mid-turn engine restart can still `--resume` (the resume/recovery lookups read `cc_session_id` from this event as well as from `CodingAgentIdled`). | Once at Init + on each user toggle |
| `CodingAgentPermissionRequest` | CC's MCP permission-prompt subprocess asked to confirm a tool call (Edit/Write/Bash on a path outside cwd, anything under `.claude/` or `.git/`). The user resolves it via `POST /api/v1/permission/<request_id>/{allow,deny}`. | Per tool call needing consent |
| `CodingAgentPermissionResolved` | The above request was answered (or auto-resolved by recovery / supersession). Carries `allowed: bool`, an optional `persist_scope` (`narrow`/`broad`/`session`) recording which "Always allow"-style scope the user picked, and a `reason` for failure / orphan-recovery / superseded cases. | Pairs 1:1 with `CodingAgentPermissionRequest`. Two engine auto-resolution paths emit `allowed: false`: orphan-recovery (`reason: "Coding agent terminated before answering — request expired"`, the agent died first) and supersession (`reason: "Superseded by a new message"`, the user typed a new message instead of clicking a button on the card) |
| `CodingAgentIdled` | **The turn-boundary marker.** Emitted at the end of every CC turn whose Result wasn't an engine-shutdown abort. See "`CodingAgentIdled` semantics" below for the full payload. | One per turn (a session normally has 1–3 across its life; many more if the user keeps replying) |
| `MissingHardeningDetected` | Engine detected that a CC session ended without running the required `/harden` and auto-spawned a recovery hardening session. **Not a session terminator** — the thread stays active until hardening finishes. | Rare; only on the recovery path |

### Persisted, high-volume — pair with `condition:` (or, for streaming, blocked entirely)

These fire many times per turn. `CodingAgentTextStreamed` is per-token streaming and is **blocked** by the scheduler — subscribing is a no-op. `CodingAgentToolCalled` / `CodingAgentToolResult` are per-tool-call (a few to a few dozen per turn): they flow through the matcher, but a trigger without a `condition:` filter will fire on every tool call. Always scope (e.g. `name: "Bash"`, `args.command: { $regex: "rm -rf" }`).

| Event | When it fires | Triggerable |
|---|---|---|
| `CodingAgentTextStreamed` | Each `text` chunk CC streams to the user. One per assistant-message line / paragraph as CC writes it. | **no (blocked — per-token streaming)** |
| `CodingAgentToolCalled` | Each tool invocation CC makes. Carries `name`, `args` (full JSON), optional `description`, and `tool_use_id` so the matching `ToolResult` can be paired even when a permission prompt splits them across exchanges. | yes (use condition) |
| `CodingAgentToolResult` | The result returned to CC for a prior `ToolCalled`. Carries the same `tool_use_id`. | yes (use condition) |

### Transient — never persisted, broadcast over SSE only

| Event | When it fires |
|---|---|
| `CodingAgentThreadSpawned` | A child CC thread (spawned via `run_claude` / `run_thread`) has started — carries the new `cc_thread_id` + `title`. SSE-only: the persisted record of the child is its own thread row. |

### `UserQuestion*` — the question / permission channel

These are NOT prefixed `CodingAgent*` because the same machinery serves any agent that needs to ask the user a structured question. Today three raise paths exist: CC's built-in `AskUserQuestion` tool, the MCP permission prompt (see "Question vs. permission" below), and the chat agent's `ask_user_question` LLM tool. All three emit the same `UserQuestionAsked` event and all three route their answer through `POST /api/v1/threads/{thread_id}/answer-question`; the engine branches on `meta.channel` to fire the right resume side-effects (CC: respawn with `--resume` + tool_result; chat: wake the in-process tool waiting on the question wait registry).

| Event | When it fires |
|---|---|
| `UserQuestionAsked` | An interactive question has been raised — typically by CC's `AskUserQuestion` tool, the chat agent's `ask_user_question` tool, or a permission-prompt path that routes through the same registry. `meta.channel` records which agent raised it (`claude_code` / `chat`). For CC the subprocess is killed at intercept; for chat the in-process tool blocks. Resume happens via `POST /api/v1/threads/{thread_id}/answer-question` (which emits `UserQuestionAnswered` and dispatches the channel-specific resume path). |
| `UserQuestionAnswered` | The user (or, on the orphan-recovery path, the engine) supplied an answer. Pairs 1:1 with the matching `UserQuestionAsked` via `tool_use_id`. Carries the originating channel via `meta.channel` so downstream consumers can tell chat answers from CC answers without re-looking-up the `Asked`. |

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
    "coding_agent": "ClaudeCode",
    "worktree_path": "/Users/.../.lucidos/worktrees/thread-1a2b3c4d",
    "worktree_head_sha": "f6ae7364e…",
    "bg_bash_pending": false
  }
}
```

All fields except `coding_agent` are `#[serde(skip_serializing_if = ...)]`-gated and will be missing from the wire when at their zero value (`false` for bools, `None` for `Option`s). Read defensively: `payload.has_changes ?? false`, `payload.worktree_path ?? null`.

| Field | Type | When present |
|---|---|---|
| `has_changes` | `bool` | `true` iff the CC branch has a non-empty net diff against its **diff base** — the SAME base the Diff button renders against (`default_diff_base`: `origin/<default>` when the local default branch has diverged, otherwise the local default) — **after** filtering out runtime-only paths (`.lucidos/**` etc. — see `branch_changed_files` + `files_require_restart`). Sharing the base is deliberate: the gate that shows the Diff button and the algorithm that computes the diff are one (`branch_changed_files`), so the button can never light up on an empty diff. Carries forward from a prior idle if the live turn produced no new commits but the branch still has prior work. Drives the `coding_agent_has_diff` projection column (the WaitingBanner Diff button); `coding_agent_proposed` is still set exclusively by the aggregate `ChangeProposed` (non-empty `change_id`), which only follows when CC's turn ended `Generated` and `should_propose_change_at_idle` permits. Aborts / cancels / mid-turn deaths never set the chip — see the "Changes" section of `thread-events.md`. |
| `is_external_repo` | `bool` | `true` for sessions running against a repo imported via `RepositoryImported` (i.e. not the Lucidos engine repo itself). Authoritative for the `coding_agent_is_external_repo` column the WaitingBanner reads to swap Apply for Done/Archive. |
| `requires_restart` | `bool` | Derived from the same filtered file list — `true` iff at least one changed file matches `files_require_restart` (Rust source, `Cargo.lock`, certain bundled assets). Informational only — `ChangeProposed.requires_restart` is the authoritative source for the `coding_agent_requires_restart` column. |
| `cc_session_id` | `Option<String>` | The CC CLI session id at the moment of idle. `None` for recovery-emitted idles where no live subprocess existed (see "no-branch" and "stuck-session" recovery paths). Not the only carrier: the id is also pinned at `Init` via `CodingAgentSettingsChanged`, so a turn interrupted by an engine restart before its first idle is still resumable — `lookup_latest_cc_session_id` reads the most recent non-null id across both event types. |
| `coding_agent` | `CodingAgent` | `"ClaudeCode"` or `"Codex"`. Defaults to `"ClaudeCode"` on legacy DB rows. Has `#[serde(alias = "agent")]` so rows persisted under the older `agent` field name still decode. |
| `reason` | `Option<String>` | **Usually absent.** Stamped only by recovery: `"engine_restart_interrupt"` when a mid-turn-crashed session is surfaced to the UI as "interrupted, click to continue" instead of being auto-spawned. The frontend reads this to render the continue affordance. |
| `worktree_path` | `Option<String>` | Absolute filesystem path of the worktree the agent ran in. Populated by `run_session.rs` for normal turns. **`None`** when the worktree was `worktree remove --force`'d before the idle fired (the "stale session" cleanup path, the no-branch recovery path) and on legacy rows. |
| `worktree_head_sha` | `Option<String>` | Snapshot of `git rev-parse HEAD` in the worktree at idle time, used by the next spawn to detect external user edits made between turns. `None` on legacy rows, when no worktree, or when `git rev-parse` fails (e.g. zero-commit branch). |
| `bg_bash_pending` | `bool` | **Recorded history only — no longer gates proposal or drives UI.** `true` iff the turn ended idle while the chat-agent's `run_bash_background` tool still had a task running. As of the bg-bash-gate removal, `should_propose_change_at_idle` ignores background bash entirely: the change proposes the instant CC idles, and correctness is covered by harden-at-apply (an un-hardened change re-runs `/harden`, tests included, before it can merge). The earlier scheme — a projection column + "CC waiting on background tasks…" banner + a 5-minute `BgBashWakeRequested` nudge — was removed because that wait was worse than the rare wasted re-harden it prevented (it also wedged permanently when CC verified completion via a shell `ps` check instead of `TaskOutput`). The field is kept on the event so the timeline still records that an idle overlapped a background task. **Startup recovery:** any idle CC thread with a committed diff but no proposed change (e.g. one wedged by the old gate) is re-proposed at boot by `propose_held_back_changes_on_startup` (skip if already-proposed / no diff / external-repo / branch missing), emitting `ChangeProposed` with `origin: Engine{ reason: StaleSession }`. |

**Fires on:** every CC turn that ended on a `Result` other than engine-shutdown abort. That covers natural completion, `Failed` (CC errored, OOM, empty assistant text), and user `Cancel` (the `Stop` button — cancel is treated as a turn boundary, not a terminator). Engine shutdown does NOT emit `CodingAgentIdled` — recovery resumes the session on next start.

**Cancel = Esc (resumable):** the `Stop` button is a real *interrupt*, not a kill. `POST /api/v1/claude-code/stop` (default, `StopReason::UserStop`) routes through `interrupt_agent`, which forwards CC's native interrupt (the equivalent of pressing `Esc` in the CLI). CC winds down the current turn, emits a `Result`, and the engine emits `ResponseCanceled(UserStop)` **+** `CodingAgentIdled` carrying the `cc_session_id`. The branch is **kept** even with zero commits (`SessionEndAction::KeepCanceledBranch` in `finalize`), so the next message `--resume`s the *same* conversation on the *same* branch — no fresh session, no re-asking. (Apply / Discard / Archive still hard-stop via `stop_agent`; each carries its own terminator.) A bounded fallback escalates to the hard stop only if CC fails to honor the interrupt within ~8s (hung socket, control request ignored while a long tool runs — the watchdog skips while a tool is in flight); even then the turn is stamped `Canceled(UserStop)` so the branch is kept and the session stays best-effort resumable. Before this, the `Stop` button hard-killed CC and `git branch -D`'d the branch, so the follow-up spawned a brand-new, amnesiac session.

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
| `tool_use_id` | `String` | The agent's identifier for this question; used as the unique key in DB and in `UserQuestionAnswered.tool_use_id`. Per-question sub-ids are synthesised as `{outer}#q{i}` so a multi-question batch from a single tool call gets one DB row per individual card. |
| `cc_session_id` | `String` | The CC session id at the moment of intercept — resume pins to it via `--resume`. **Empty string** for questions raised by the chat agent's `ask_user_question` tool (chat is in-process, no subprocess to resume). |
| `question` | `String` | The prompt text shown to the user. **For permission requests, this is the human-readable rendering of the tool-use that CC asked permission for** — see "Question vs. permission" below. Populated strictly from the tool input's `question` field; the optional `header` chip-label is never accepted as a substitute. The schema marks `question` `required`, but tool-call schemas are advisory (the model can still omit it — that was the "(no question text)" bug), so the engine enforces it: a batch with any entry missing a non-empty `question` is rejected before any `UserQuestionAsked` is emitted, bouncing it back to the model to re-ask. This field is therefore never empty / `(no question text)`. |
| `options` | `Vec<QuestionOption>` (default `[]`) | Each option is `{ id, label, description? }`. Empty for free-text-only prompts. |
| `worktree_path` | `Option<String>` | The CC worktree at intercept. Required for `--resume` to find the session JSONL (CC keys session storage by CWD). `None` for chat-channel questions and when the request came in without a worktree context. |
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

`persist_scope` is `"narrow"` / `"broad"` / `"session"` when the user clicked "Always allow"-style; `null` for plain Allow-once / Deny / orphan-recovery / supersession. The frontend reads it to render the answered card with a check on the chosen button and strike-through on the rest.

**Supersession.** If the user types a new message while a permission card is still pending, the engine resolves the card as `allowed: false` (`reason: "Superseded by a new message"`) before routing the typed text to CC as a normal follow-up — so the buttons stop dangling instead of leaving the thread stuck on `waiting_for_user_answer` while CC moves on. This mirrors the AskUserQuestion free-form path, where typing becomes a `FreeText` answer; permissions have no "answer", so the pending request is denied instead. Emitted from `resolve_pending_permissions_as_superseded` (`engine/cc_permission.rs`).

## Question vs. permission — and why both event families exist

There are three distinct user-facing prompt mechanisms in play, riding on two event lanes:

1. **CC's native `AskUserQuestion` tool.** A first-class tool CC can call to get structured input mid-turn ("which approach", "is this OK to do"). Fires `UserQuestionAsked` / `UserQuestionAnswered` with `meta.channel = claude_code`. The `question` payload field is the literal prompt text CC supplied.
2. **Chat agent's `ask_user_question` tool.** Same wire shape, same QuestionCard rendering, but raised by the in-process chat agent rather than a CC subprocess. Fires `UserQuestionAsked` / `UserQuestionAnswered` with `meta.channel = chat`. The chat tool blocks on the question wait registry until the answer arrives, then returns the joined `{question_text: label}` map as the tool result on the same turn. Use this from chat threads whenever a button-driven answer beats forcing the user to type — see `crates/lucidos-engine/src/llm/tools/mod.rs` § `ASK_USER_QUESTION`.
3. **MCP permission prompts.** A separate subprocess CC consults whenever it would invoke `Edit`/`Write`/`Bash` on a path it doesn't have a static allow rule for. Fires `CodingAgentPermissionRequest` / `CodingAgentPermissionResolved`. The user is choosing whether to authorize a specific tool call, possibly with persistence ("Always allow Bash(git:*)").

These look similar in the UI (both render as a card the user has to act on) but they are two separate event lanes. **A "permission prompt" is not a third event family**; it's whichever of these two raised it.

In practice today:

- A workspace LLM that says "notify me when Claude Code is asking for permission" almost always means *either*: it should subscribe to `UserQuestionAsked` (covers the AskUserQuestion case and any permission-style prompt that the engine routes through the question registry), *or* — for true MCP permission prompts — `CodingAgentPermissionRequest`. Pick based on what the user actually wants to act on.
- `CodingAgentPermissionRequest` is also triggerable — both blocking-request paths flow through the matcher, so you can pick whichever matches your case (or wire a separate trigger to each).
- Lucidos does NOT seed a default question-push trigger. Workspaces opt in by creating a user trigger themselves (see the worked example in `building-a-trigger.md` § "Worked example — push when CC needs me").

## The error gap

There is no `CodingAgentErrored`, `CodingAgentFailed`, or `CodingAgentCrashed` event. Verified via `rg -n "CodingAgent(Error|Errored|Failed|Crashed|Aborted|Canceled)" crates/lucidos-engine/src/` — zero matches.

When a CC session fails mid-turn (upstream API error, OOM-killed bash, empty assistant text on a non-cancel turn), the engine routes it through `classify_result` (`agent_session/lifecycle.rs`) and:

- emits a chat-side `ResponseFailed { error }` to mark the turn as failed;
- emits `CodingAgentIdled { has_changes: <whatever was on the branch> }` so the dispatcher closes the turn and the UI exits the "Working" state.

`ResponseFailed` and `CodingAgentIdled` are both triggerable today. The cleanest wiring for "notify me when Claude Code errors" is:

```yaml
on:
  - event_type: ResponseFailed
    # Optional: scope to coding-agent threads. ResponseFailed payload itself
    # has no channel field; if you only want the CC subset, fire a domain
    # event from the failure path instead (see option 3) or layer a separate
    # CodingAgentIdled subscription.
run:
  intent: "Send me a push notification that the response failed."
```

Or, if you specifically want "CC turn ended without a clean response":

```yaml
on:
  - event_type: CodingAgentIdled
    condition:
      has_changes: false
      reason: "engine_restart_interrupt"
run:
  intent: "Tell me CC needed engine recovery and is paused for me to restart."
```

For a workspace-defined failure name, `lucidos events emit CodingAgentFailureObserved {...}` (or the `emit_event` LLM tool) writes a `SystemEvent::DomainEvent` that flows through the matcher unconditionally. Use this when the engine's own ThreadEvents don't carry the discriminator you need (e.g. you want to distinguish OOM from API-503 in the trigger).

## Notes on `CodingAgentPromptSent`

This is NOT the user typing into CC. User-typed input fires `CodingAgentUserMessageSent`. `CodingAgentPromptSent` is for prompts the engine itself synthesized:

- merge-conflict explanation injected after `MergeConflictDetected`,
- `MissingHardeningDetected` recovery prompt asking CC to run `/harden`,
- the empty `CodingAgentPromptSent` emitted right after `UserQuestionAnswered` so the timeline shows a "thinking" placeholder while CC processes the answer. Skipped when `answer.kind == "Canceled"` — the cancel-stamp path (`claude_code_stop`, `archive_thread`) tears CC down right after, so no resume turn ever runs and the marker would strand as an empty `Thinking ✓` step under the QuestionCard's own `✓ Cancel` state. See `emit_resume_marker_for_cc_answer` in `crates/lucidos-engine/src/engine/agent_question.rs`.

Distinguishable on the wire by `origin: Some(MessageOrigin::Engine { reason: ... })`. Workspaces should generally not need to subscribe to this — it's an audit trail event, not a lifecycle signal.

## Recipe-shaped guidance

For the trigger config field reference (cron format, the `on` subscription list, per-entry `condition` operators), see `system-knowhow/building-a-trigger.md`. The condition language is `$eq` / `$ne` / `$lt` / `$lte` / `$gt` / `$gte` / `$in` over top-level payload fields (a bare value is `$eq`); see `crates/lucidos-engine/src/triggers/condition.rs` for the full operator set.

### Notify when CC is waiting on the user

```yaml
on:
  - event_type: UserQuestionAsked
run:
  intent: "Notify me when Claude has a question waiting for me. The push should deep-link straight to the question — tapping it takes me to the originating thread and pulses the question card on land."
```

The `tap` + `event_id` combination is what makes the push actually deep-link: the push tap navigates straight to the originating thread *and* the matching event card pulses on land. Without them the push defaults to opening the inbox modal. See `building-a-trigger.md` for the full pattern.

To scope to a specific CC session, add a per-entry `condition`:

```yaml
on:
  - event_type: UserQuestionAsked
    condition:
      cc_session_id: "abc123-…"
```

Conditions are pure payload filters — they only look at the event's payload fields. `cc_session_id` is on `UserQuestionAsked`'s payload (see shape above), so this works. There is no conditional access to thread metadata, app id, etc. from the condition.

### Notify when a CC session finishes / produced changes

```yaml
on:
  - event_type: CodingAgentIdled
    condition:
      has_changes: true
run:
  intent: "Tell me Claude Code finished and left a change to review."
```

Adding `is_external_repo: { $ne: true }` scopes to the engine repo; `requires_restart: true` scopes to changes that need an Apply & Restart, etc. — same condition language across all per-payload fields.

### Notify when CC errors

```yaml
on:
  - event_type: ResponseFailed
run:
  intent: "Send me a push notification with the failure error."
```

`ResponseFailed` fires for both chat and CC failures — see "The error gap" above for what "error" means in this codebase and the alternatives if you need finer discrimination.
