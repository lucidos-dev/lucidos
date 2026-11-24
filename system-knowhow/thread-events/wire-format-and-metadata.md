---
name: ThreadEvent Reference, Wire format and metadata
description: Part of ThreadEvent Reference. Its index lists the sections it covers.
---

One part of `system-knowhow/thread-events`, which lists every part.

## Wire format and metadata

Persisted events are stored with `event_type` set to the variant name and `payload` as the variant's JSON object. `EventMeta` merges cross-cutting fields into the payload at persist time (`EventMeta::apply` in `crates/lucidos-core/src/engine/thread_events/meta.rs`):

- `request_event_id`: links response and terminal events back to the originating request.
- `channel`: `"chat"` / `"claude_code"` / `"trigger"` (`EventChannel`). `"claude_code"` is the coding-agent channel for both Claude Code and Codex, kept for compatibility with existing rows and clients.
- `actor`: the `MessageOrigin` of who initiated. Stamped by mutating HTTP handlers via `api/actor::user_actor`.

Some variants (`ChangeApplied`, `ChangeDiscarded`, `ChangeReverted`, `ChangeApplyFailed`, `ChangeHardened`, `ThreadStarted`, `ThreadDiscarded`, `ImageUploaded`) carry `actor` as their own field, which predates `EventMeta`. `MessageReceived` and several others use `origin: Option<MessageOrigin>`. Treat either as that event's canonical "who did this" field.

### Saved-draft origins: `origin.kind == "saved_draft"`

Saved owner content carries Human mode with a distinct origin (ADR 0651). `submission_event_id` names its acceptance audit; `submitted_by_thread_id` names the agent that submitted it. This origin is never Device and grants no fresh owner-opened turn or wider spawn authority. An agent submits through `lucidos threads submit-draft`, the `threads` tool's `submit_draft` action, or `POST /api/v1/threads/:thread_id/submit-draft`.

### Another workspace: `origin.kind == "workspace"`

A thread another workspace spawned carries `origin: { "kind": "workspace", "workspace", "thread_id"?, "mode", "verdict"? }`. The gateway vouched for it. `verdict` is the calling workspace's own coding-agent check on its thread: `{ "kind": "allowed" }`, `{ "kind": "refused", "reason" }` or `{ "kind": "unattested" }`. A row with no `verdict` predates the check.

A spawn the gateway did not vouch for is `{ "kind": "unattested_caller", "claimed_workspace" }` instead. Its claimed name is a hint, never proof. Optional `claimed_thread_id` preserves the named thread as a display hint, never authority. Old events omit it.

### Engine origins: `origin.kind == "engine"`

When the engine writes a message or prompt, the event carries `origin: { "kind": "engine", "reason": { "kind": … } }` and `mode: "engine"`. The reason says why. The one exception is an event-wait re-entry anchor, which keeps `mode: "agent"` (§ The re-entry anchor). Two reasons seed a whole thread and name what the engine acted on:

| `reason.kind` | When | Fields |
|---|---|---|
| `plugin_setup` | The first message of a *setup thread*, after the user confirmed an install or update with new setup instructions or a raised `setup_revision`. | `plugin_id`, `plugin_name`, `version` (just installed), `occasion` (`{ "kind": "fresh_install" }` or `{ "kind": "update", "from_version"?, "cause"? }`, where `cause` is `instructions_changed` (also when absent), `revision_raised` or `both`, the last two carrying `revision` and an optional `reason`), optional `confirmed_on_device_id` |
| `plugin_upstream_proposal` | The first message of a thread that offers the user's local plugin edits to the plugin's author. | `plugin_id`, `plugin_name`, `version`, `patch_path`, optional `confirmed_on_device_id` |

The engine wrote the words, so the engine is the origin, never the user's device. The confirming device rides beside it as `confirmed_on_device_id`, the id only. `from_version` is absent when the prior install record names no version.

The complete reason kinds, including historical values, are:

<!-- generated:begin EngineReason -->
EngineReason wire values:

- `continuation_started`
- `orphan_recovery`
- `scheduler`
- `harden_retrigger`
- `stale_session`
- `archived_branch_work`
- `unmerged_integrated_work`
- `successor_residue`
- `successor_session`
- `merge_conflict`
- `main_check_red`
- `main_check_flaky`
- `missing_hardening`
- `missing_plan`
- `approved_plan_unstarted`
- `model_substituted`
- `context_ceiling`
- `plugin_auto_update`
- `plugin_setup`
- `plugin_upstream_proposal`
- `event_wait`
- `cache_keep_alive`
<!-- generated:end -->

`scheduler` carries `trigger_id`, optional `trigger_name`, and optional `actor_email` (`system-knowhow/triggers/actor.md` § "Actor").

`archived_branch_work` identifies unproposed work recovered from an archived thread.
`unmerged_integrated_work` identifies an integrated child's work omitted from its parent's landing.
`successor_residue` identifies work left behind by a legacy successor thread.
`successor_session` identifies a handoff opening in the same thread.

`missing_plan` identifies a prompt asking a coding agent to satisfy the plan gate.
`approved_plan_unstarted` identifies a prompt telling a coding agent to build the plan the user approved, after a turn that changed no source file.
`model_substituted` marks the *model-swap re-entry*: Claude Code swapped the pinned model. The engine asks the agent to put two questions to the owner: which rung to use instead, and whether to take the model off the ladder (`system-knowhow/coding-agent-events/the-full-enumerated-list.md` § Model swaps).

`missing_hardening` marks the boundary before an *Apply* hardens a change. It also marks the engine's message to a coding agent whose turn-end work it withheld because the work never ran `/harden`. `plugin_auto_update` is historical and no longer written.

`cache_keep_alive` marks a *cache keep-alive*: the engine resumed an idle Claude Code session that waits, only to keep its prompt cache warm. The turn leaves no trace. The thread keeps its status, section, `last_activity`, `message_count` and read decision, and no parent, wait or trigger hears its idle. A real input waits the few seconds until the keep-alive ends, then opens its own turn. The keep-alive as a whole shows through `CacheKeepAliveStatusChanged` (§ The cache keep-alive status).

`context_ceiling` marks the *ceiling re-entry*: the session grew past its *context ceiling*, so the engine asked the agent to write a handoff narrative and hand off (ADR 0629). The earlier `ContextCeilingReached` names the size and ceiling at the crossing.

`merge_conflict` marks the prompt of a merge-conflict resolution: an app change's Tier-2 or Tier-3 merge, or a turn the engine sends a *lane thread*.

`main_check_red` marks the first prompt of a red main repair. `main_check_flaky` marks a repair of flaky findings without red. Both start only after idle admission or the owner's Fix now. `MainCheckRepairStarted` records the owner and admission before the prompt.

Every thread the engine spawns carries an origin or a parent link, so its first message says who started it. Older rows may show no origin.

## Volume classes

Every table below uses these. Pick the right class before subscribing a trigger.

- **lifecycle**: fires once at a moment in a thread's life (creation, archive, terminal). Safe to trigger on directly.
- **one-per-turn**: fires at most once per chat or coding-agent turn. Safe to trigger on, with a `condition` if needed.
- **per-action**: fires once per discrete action (one tool call, one message, one captured context). Triggerable, but always add a `condition`, or it fires the trigger many times per turn.
- **high-volume-streaming**: many fires per turn (per token chunk). **Blocked by the scheduler** (`TextStreamed`, `ThoughtStreamed`, `CodingAgentTextStreamed`, `CodingAgentThoughtStreamed`); the matcher never sees them.
- **transient-SSE-only**: never persisted; cannot trigger.

## Chat / agentic loop

These fire on chat threads (`channel = chat`) and trigger runs (`channel = trigger`). The coding-agent equivalents use the parallel `CodingAgent*` names.

