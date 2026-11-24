---
name: ThreadEvent Reference, Resume / continuation
description: Part of ThreadEvent Reference. Its index lists the sections it covers.
---

One part of `system-knowhow/thread-events`, which lists every part.

## Chat / agentic loop (continued)

**One turn, one terminator, even during a restart.** An interruption produces two would-be terminators. The engine pre-emits `ResponseAborted` for the turn it tears down, and the loop's cancel arm fires moments later. Both name the same turn through `request_event_id`, so `has_terminator_for` sees the abort and `emit_response_canceled` skips. That holds only because the pre-emit takes the anchor from the running turn (`engine::in_flight_request_event_id`), not the newest `MessageReceived`. Guessing stacked "Paused by restart" on "Response canceled" after a mid-turn follow-up or a `ContinuationStarted` turn.

The rule covers all three out-of-loop emitters: the `/api/v1/restart` teardown, the 60 s stuck-turn eviction, and the shutdown sweep.

**A coding-agent session needs another way, because its request id cannot do that job.** A live session keeps ONE `request_event_id` across every follow-up turn. So "a terminator exists for this request id" would read turn one's `ResponseGenerated` as covering turn two, and swallow a real terminator. Its suppression is scoped to the *turn* instead: a `ResponseAborted` that out-sequences the thread's newest start event already covers the turn, so the session emits nothing. Start events are `MessageReceived`, `CodingAgentUserMessageSent`, `TriggerStarted` and `ContinuationStarted`. The query also recognizes the retired event `OrphanRecoveryStarted` on stored rows.

Two checks apply. The in-memory `AgentSession::external_terminal_emitted` flag is the fast path when the session was already registered. The events-table check covers a boundary emitted **before the session existed at all**. Both restart emitters iterate a snapshot of `agent_sessions`, so they miss a session still spawning. Without that check, a *Switch to new version* during a spawn stacked a `ResponseCanceled{user_stop}` beside "Paused by restart". The resume gate reads a cancel as "turn ended", so the next boot declined the promised auto-resume.

## Resume / continuation

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `ContinuationStarted` | Resume-after-abort boundary. Opens a new exchange whose body is the rerun (chat: a fresh LLM call; coding agent: resume into the same `cc_session_id` when the backend has one). **Channel-agnostic**: emitted on chat, trigger and coding-agent paths alike, so it says nothing about a thread's type (see below). Carries optional `branch`, engine-stamped `origin`, `reason`, which mirrors `ContinuationRequested.reason` so a hang or stray-signal recovery is not labelled an engine restart, and `resumed_abort_id`, the abort a chat or trigger Continue resumes. Aliases for old DB rows: `SessionRecovered`, `SessionResumed`. | lifecycle | yes | yes |
| `SessionStarted` | Optional `sandbox` records the resolved spawn mode; see the generated values below. Absent on legacy rows.  A coding-agent process spawned. Carries `session_id` (backend session id), optional `branch`, optional `repo_id`, plus *coding-agent-thread* discriminators. `coding_agent_kind`: `"lucidos" \| "app" \| "external"`, default `"lucidos"`. `coding_agent_folder`: the folder the spawn targets, `<ws>/data/apps/<id>/` for App, else the repo root. `app_id`: set only for App. `coding_agent`: `"claude-code" \| "codex"`, default `"claude-code"`, the backend driving this session. `thread_summaries.coding_agent` follows the newest SessionStarted, so a *successor session* may switch it. `integrated: true` (omitted when false) marks an *integrated sub-thread*, kept for the thread's life in `thread_summaries.coding_agent_integrated`. `auto_compact_arm` (`"control" \| "window" \| "override"`) and `auto_compact_window` (tokens) record the auto-compact trial on a Claude Code spawn. Both are omitted when the trial is off and on Codex. The window is omitted under `control`, and under `override` when the workspace value is not a number. `continuity` says whether the session carried the thread's earlier context; see the generated values below. Its `handoff` kind marks a *successor session*, with the opening it started from, and the `checkpoint` and `progress` it handed off at when there was one. On a handoff at the *context ceiling* it also carries `crossing`, as the handoff recorded it. The transcript draws its card from this field. Omitted on rows written before it existed. Legacy rows without these decode as Lucidos / Claude Code / independent via serde defaults. It leaves the thread's compose draft and *compose epoch* alone: the send that started the session consumed them when the engine accepted it. | lifecycle | yes | yes |
| `SessionEnded` | A coding-agent thread is truly done (terminal-only). Carries `reason: SessionEndReason`; see the generated values below. `stale_resume` is the one transient case and does NOT settle the thread. The projection stays `running` while the caller re-spawns once with a fresh session (chat and the continuation/spawn consumer both do), and the frontend skips the AbortPanel. A caller that does not retry leaves the thread `running` with no subprocess, so the retry is part of the contract. | lifecycle | yes | yes |

The session payload enums use these wire values. `SessionContinuity` is tagged by `kind`.
Its `fresh` variant carries a `FreshSessionReason` in `reason`.
Its `handoff` variant carries a `HandoffOpening` in `opening` and the opening text in `message`.

<!-- generated:begin SandboxMode -->
SandboxMode wire values:

- `enforced`
- `inherited`
- `owner-opted-out`
- `permitted-outside`
<!-- generated:end -->

<!-- generated:begin SessionEndReason -->
SessionEndReason wire values:

- `shutdown`
- `panic`
- `closed`
- `stale_resume`
- `legacy_non_terminal`
<!-- generated:end -->

<!-- generated:begin SessionContinuity -->
SessionContinuity wire values:

- `first`
- `resumed`
- `fresh`
- `handoff`
<!-- generated:end -->

<!-- generated:begin FreshSessionReason -->
FreshSessionReason wire values:

- `change-applied`
- `change-discarded`
- `session-ended`
- `branch-gone`
- `resume-failed`
- `no-resumable-session`
- `provider-blocked`
<!-- generated:end -->

<!-- generated:begin HandoffOpening -->
HandoffOpening wire values:

- `narrative`
- `brief`
<!-- generated:end -->

`SessionHandedOff` carries a `HandoffCause` in `cause` and a `SessionStart` in `starts`.

<!-- generated:begin HandoffCause -->
HandoffCause wire values:

- `checkpoint`
- `user`
- `context-ceiling`
<!-- generated:end -->

<!-- generated:begin SessionStart -->
SessionStart wire values:

- `now`
- `after-this-turn`
- `on-next-input`
<!-- generated:end -->

## Coding agent (Claude Code / Codex)

The `CodingAgent*` family covers Claude Code and Codex. Variants carry `coding_agent: CodingAgent`, default `ClaudeCode`, with `#[serde(alias = "agent")]` so pre-rename rows decode. Each variant has a `#[serde(alias = "ClaudeCode<X>")]` for its pre-rename name. Write new code against the `CodingAgent*` form.

**`system-knowhow/coding-agent-events.md` has the full payload shapes, the `CodingAgentIdled` field reference, the `UserQuestion` vs `CodingAgentPermission` distinction, and the no-`CodingAgentErrored` gap.** This table is the index.

