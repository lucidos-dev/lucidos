---
name: ThreadEvent Reference
description: The full enumerated list of `ThreadEvent` variants the Lucidos engine emits — chat-side (MessageReceived, ResponseGenerated, …), coding-agent (CodingAgent*), thread lifecycle (ThreadStarted, ThreadArchived, …), changes (ChangeProposed, ChangeApplied, …), background bash, plugin / repo / merge-conflict, transient SSE-only commands (RefreshAppUI, CaptureAppUI, …). Documents each variant's payload, volume class, whether it's persisted, and whether a workspace `on_event:` trigger can subscribe to it today (the scheduler's ThreadEvent allowlist is gated to one entry). Load when the user asks "what events fire on a thread", "can I trigger on ResponseGenerated / ChangeApplied / TriggerCompleted / BackgroundBashCompleted", "list every ThreadEvent", "is X event persisted", "what payload does Y carry", or talks about the EventBus, event sourcing, SSE event stream, on_event triggers, projections — and especially before wiring `on_event:` to anything other than `UserQuestionAsked`. For the CC-only deep-dive (UserQuestion vs permission distinction, CodingAgentIdled semantics, the no-`CodingAgentErrored` gap), see `system-knowhow/coding-agent-events.md`.
---

# ThreadEvent Reference

The complete enumerated list of `ThreadEvent` — the per-thread event family that flows through the EventBus into PostgreSQL, the SSE stream, and (for one curated entry) the trigger matcher. Source of truth: `crates/lucidos-engine/src/engine/thread_events.rs`. Variant names below are the **current** names; legacy aliases (e.g. `ClaudeCodeIdled`, `SessionRecovered`, `parent_thread`, `task_id` / `task_name`) exist as `#[serde(alias = ...)]` on the wire so old DB rows decode cleanly — write new code, new triggers, and new docs against the current name only.

For the CC slice (`CodingAgent*` + the `UserQuestion*` / permission machinery) the deep-dive lives in `system-knowhow/coding-agent-events.md`. This file is the master enumeration; the CC entries below summarize and link.

For event-store column shape, the chat-mode terminator set, and the `events` table schema, see `.claude/rules/db.md`. For trigger config syntax (cron, `on_event`, `condition` operators), see `system-knowhow/building-a-trigger.md`.

## CRITICAL: today only `UserQuestionAsked` actually fires triggers

The scheduler subscribes to the EventBus and forwards events to the trigger matcher, but the `BusEvent::Thread` branch is gated by an explicit allowlist (`crates/lucidos-engine/src/scheduler/mod.rs`, look for `// Allow a curated subset of ThreadEvents`). Today that allowlist contains exactly one entry: `UserQuestionAsked`.

That means right now:

- `on_event: UserQuestionAsked` — works.
- `on_event: <any other ThreadEvent>` — **does not fire today**. The trigger config will validate and the trigger row will be persisted, but the matcher will never see the event because the scheduler skips it. This includes `ResponseGenerated`, `ResponseFailed`, `CodingAgentIdled`, `ChangeApplied`, `TriggerCompleted`, `BackgroundBashCompleted`, every `Change*`, every `Thread*` lifecycle event, etc.
- `on_event: <a workspace-emitted DomainEvent>` — works. `SystemEvent::DomainEvent` (emitted via `lucidos events emit` / `emit_event` LLM tool) DOES go through the matcher. This is the supported path for "trigger on something my workspace observes."

If a workspace asks for "notify me when X" and X is a `ThreadEvent`, **first** add it to the scheduler allowlist (engine code change, not a workspace config), or arrange for the relevant code path to also emit a domain event the trigger can listen to. Don't ship a trigger that silently never fires. The `In allowlist` column on every table below is the binary "would a trigger fire on this today?" answer.

## Persisted vs transient

The enum splits into two halves, mirrored by `ThreadEvent::is_persisted()`:

- **Past tense → persisted.** `MessageReceived`, `ResponseGenerated`, `CodingAgentIdled`, `ChangeApplied`, etc. Written to the `events` table; replayable; visible to projections, history queries, and (in principle) the trigger matcher.
- **Present participle / imperative → transient.** `TextStreaming`, `Retrying`, `RefreshAppUI`, `PluginInstallRequest`, etc. Broadcast over SSE only. Never persisted; never reach the projection or trigger paths. Used for live UI updates (token streaming, modal-trigger commands) and child thread broadcasts.

A trigger on a transient event can never fire — it isn't even allowlistable, because the scheduler's matcher only looks at persisted events.

## Wire format and metadata

Persisted events are stored with `event_type` set to the variant name and `payload` as the variant's JSON object. Cross-cutting fields merged into the payload at persist time by `EventMeta` (see `engine/thread_events.rs` `EventMeta::apply`):

- `request_event_id` — links response/terminal events back to the originating request.
- `channel` — `"chat"` / `"claude_code"` / `"trigger"` (`EventChannel`). The `"claude_code"` wire string is the legacy name for the CC channel; rename pending coordinated migration.
- `actor` — `MessageOrigin` of who initiated. Stamped by mutating HTTP handlers via `api/actor::user_actor_resolved`.

Some variants (`ChangeApplied`, `ChangeDiscarded`, `ChangeReverted`, `ChangeApplyFailed`, `ChangeHardened`, `ThreadStarted`, `ThreadDiscarded`, `ImageUploaded`) carry `actor` as a per-variant field (predates `EventMeta`); `MessageReceived` and several others use `origin: Option<MessageOrigin>`. Treat both as the canonical "who did this" field for that event.

## Volume classes

Used in every table below. Pick the right class before subscribing a trigger.

- **lifecycle** — fires once at a moment in a thread's life (creation, archive, terminal). Safe to trigger on if the trigger is allowlisted.
- **one-per-turn** — fires at most once per chat / CC turn. Safe to trigger on (allowlisted), with a `condition` if needed.
- **per-action** — fires once per discrete user/agent action (one tool call, one message, one captured context). Use a `condition` filter to scope.
- **high-volume-streaming** — many fires per turn (per token chunk, per LLM round-trip). **Do not trigger on these** even if allowlisted in future — they will saturate whatever they're wired to.
- **transient-SSE-only** — never persisted; cannot trigger.

## Chat / agentic loop

These fire on chat threads (`channel = chat`) and on trigger-driven runs (`channel = trigger`). The CC equivalents under "Coding agent" use the parallel `CodingAgent*` names.

| Event | When it fires | Volume | Persisted | In allowlist |
|---|---|---|---|---|
| `MessageReceived` | A user (or upstream workspace, or parent thread, or engine) submitted text into the thread. Stamped at the HTTP boundary in `api/chat.rs::chat_submit`. | one-per-turn | yes | no |
| `TextStreamed` | A complete chunk of assistant text was committed to the thread (post-stream finalize for chat; one event per appended chunk). | per-action | yes | no |
| `Thinking` | The model emitted a `thinking` block (extended-thinking models). | per-action | yes | no |
| `ContextCaptured` | One snapshot of the LLM context the engine assembled for a single LLM call (prompt sections, tools, estimated tokens, real `usage` when the provider reports it). One per LLM call; the modal reads these to show context drift. | per-action | yes | no |
| `MemorySearched` | The chat-side memory consumer ran a vector search to assemble context. Carries `results: usize`, `queries: Vec<String>`. | per-action | yes | no |
| `ToolCalled` | The chat agentic loop invoked a tool (`name`, `args`, optional `description`). Distinct from `CodingAgentToolCalled` — those are CC's tool calls. | per-action | yes | no |
| `ToolResult` | The result returned to the chat agentic loop for a prior `ToolCalled`. Carries `result: String`, `images`, `success: bool` (default true). | per-action | yes | no |
| `BackgroundBashStarted` | A long-running shell command was spawned via the `run_bash_background` chat tool. Paired with a later `BackgroundBashCompleted`. | per-action | yes | no |
| `BackgroundBashCompleted` | The bash task finished — natural exit, watchdog timeout, or `bash_kill`. Carries `exit_code: Option<i32>` (None on signal-only kills), `stdout`, `stderr`, `timed_out: bool`, `killed: bool`. The audit-trail counterpart of `Started`; `bash_output` falls back to this row after the in-memory registry evicts the task. | per-action | yes | no |
| `ResponseGenerated` | The chat agentic loop terminated with an assistant response. The chat-mode terminator. | one-per-turn | yes | no |
| `ResponseCanceled` | User clicked Stop, or clicked Apply / Discard / Archive on a still-running session. Carries `cause: CancelCause` (`UserStop` / `UserAction` / `Unknown`). Always emit via `thread_events::emit_response_canceled` — it's idempotent against pre-emitted terminators (the `/api/restart` race). | one-per-turn | yes | no |
| `ResponseAborted` | System-driven termination — engine shutdown, safety net, recovery sweep, OS signal, stale-projection settle. Carries `cause: AbortCause` (`EngineShutdown` / `SafetyNet` / `RecoveryAfterRestart` / `ProcessKilled` / `StaleSettle` / `Unknown`). Always emit via `thread_events::emit_response_aborted`. | one-per-turn | yes | no |
| `ResponseFailed` | Hard failure mid-turn: upstream API error, panic, OOM-killed bash, empty assistant text on a non-cancel turn (`agent_session::lifecycle::classify_result` triggers this for CC too). Carries `error: String`. | one-per-turn | yes | no |
| `UserPromptInjected` | A user correction OR an engine-injected mid-flight message (resume note, child-thread callback in legacy paths) was relayed into the live agentic loop. Carries `text`, `mode: ActorMode`, optional `origin`, optional `injected_message_id`. | per-action | yes | no |

Terminator set for chat-mode (`TERMINATOR_EVENT_TYPES` constant): `ResponseGenerated`, `ResponseCanceled`, `ResponseAborted`, `ResponseFailed`. Used by `has_terminator_for` for idempotent terminator emission.

## Resume / continuation

| Event | When it fires | Volume | Persisted | In allowlist |
|---|---|---|---|---|
| `ContinuationStarted` | Resume-after-abort boundary. Opens a new exchange in the timeline whose body is the rerun (chat: re-LLM call after abort; CC: `--resume` into the same `cc_session_id`). Carries optional `branch` and engine-stamped `origin`. Aliases for old DB rows: `SessionRecovered`, `SessionResumed`. | lifecycle | yes | no |
| `SessionStarted` | A coding-agent process spawned. Carries `session_id` (CC's CLI session id), optional `branch`, optional `repo_id`. | lifecycle | yes | no |
| `SessionEnded` | A coding-agent thread is truly done (Phase 4 of the CC resume architecture: terminal-only). Carries `reason: SessionEndReason` (`Shutdown` / `Panic` / `Closed` / `StaleResume` / `LegacyNonTerminal`). `StaleResume` is the one transient case — the chat handler retries internally; frontend skips the AbortPanel. | lifecycle | yes | no |

## Coding agent (CC / Codex)

The umbrella `CodingAgent*` family covers Claude Code and Codex (the variants carry `agent: AgentKind`, default `ClaudeCode`). Each variant has a `#[serde(alias = "ClaudeCode<X>")]` for the legacy pre-rename name — write new code against the `CodingAgent*` form.

**See `system-knowhow/coding-agent-events.md` for full payload shapes, the `CodingAgentIdled` field-by-field reference, the `UserQuestion` vs `CodingAgentPermission` distinction, and the no-`CodingAgentErrored` gap.** This table is the index.

| Event | When it fires | Volume | Persisted | In allowlist |
|---|---|---|---|---|
| `CodingAgentUserMessageSent` | A user message was relayed into the agent's input stream. | one-per-turn | yes | no |
| `CodingAgentPromptSent` | An engine-synthesized prompt was injected (orphan-recovery, hardening retrigger, merge-conflict explainer, post-question continuation). Carries `origin: Option<MessageOrigin>`. Audit-only, not rendered in chat. | per-action | yes | no |
| `CodingAgentTextStreamed` | One chunk of CC's assistant text. | high-volume-streaming | yes | no |
| `CodingAgentToolCalled` | One CC tool invocation. Carries `name`, `args`, optional `description`, `tool_use_id`. | high-volume-streaming | yes | no |
| `CodingAgentToolResult` | The result returned to CC for a prior `CodingAgentToolCalled`. Same `tool_use_id`. | high-volume-streaming | yes | no |
| `CodingAgentIdled` | **The CC turn-boundary marker.** Emitted at the end of every CC turn whose Result wasn't an engine-shutdown abort. Carries `has_changes`, `is_external_repo`, `requires_restart`, `cc_session_id`, `agent`, optional `reason`, optional `worktree_path`, optional `worktree_head_sha`. | one-per-turn | yes | no |
| `CodingAgentSettingsChanged` | User changed model, reasoning effort, or permission mode mid-session. | lifecycle (rare) | yes | no |
| `CodingAgentPermissionRequest` | CC's MCP permission-prompt subprocess asked to confirm a tool call (path outside cwd, `.claude/`, `.git/`). | per-action | yes | no |
| `CodingAgentPermissionResolved` | The above request was answered (or auto-resolved by recovery). Carries `allowed`, optional `reason`, optional `persist_scope` (`narrow` / `broad` / `session`). | per-action | yes | no |
| `MissingHardeningDetected` | Engine detected a CC session ended without running `/harden` and auto-spawned a recovery hardening session. Not a session terminator — the thread stays active until hardening finishes. | lifecycle (rare) | yes | no |
| `ContinueSignal` | Continuation marker — emitted when an interrupted CC turn (engine-restart-mid-turn, user-clicked-Continue) needs to be resumed without a new user message. Picked up by the spawn dispatcher; the event id is the spawn idempotency key. Carries `reason: String`. | lifecycle | yes | no |

## Question / permission machinery

Not prefixed `CodingAgent*` because the same machinery serves any agent that needs to ask the user a structured question. **`UserQuestionAsked` is the only `ThreadEvent` in the scheduler allowlist today** — it's how the seeded "session waiting on me" push trigger fires.

| Event | When it fires | Volume | Persisted | In allowlist |
|---|---|---|---|---|
| `UserQuestionAsked` | An interactive question raised — typically CC's `AskUserQuestion` tool, or a permission-prompt path routed through the same registry. The CC subprocess is killed at intercept; resume happens via `POST /api/cc/answer-question`. Carries `tool_use_id`, `cc_session_id`, `question`, `options: Vec<QuestionOption>`, optional `worktree_path`, `multi_select: bool`. | one-per-turn (of the `Asked` kind) | yes | **YES** |
| `UserQuestionAnswered` | The user (or, on the orphan-recovery path, the engine) supplied an answer. Pairs 1:1 with the matching `UserQuestionAsked` via `tool_use_id`. Carries `answer: AnswerKind` (`Selected` / `FreeText` / `MultiSelected` / `Canceled`). | one-per-turn | yes | no |
| `CredentialRequested` | Persisted record that a credential prompt was opened for `provider`. Pairs with the transient `CredentialRequest` SSE command that drives the modal. | lifecycle | yes | no |
| `McpConsentRequested` | Persisted record that an MCP consent prompt was opened for tool with `args`. Pairs with the transient `McpConsentRequest` SSE command. | lifecycle | yes | no |

`QUESTION_ORPHANING_EVENT_TYPES` constant (`ResponseAborted`, `ResponseCanceled`, `ResponseFailed`, `CodingAgentIdled`) — once any of these lands after a `UserQuestionAsked`, the surrounding turn is gone and the next user text starts a fresh follow-up rather than a `FreeText` answer.

## Thread lifecycle

| Event | When it fires | Volume | Persisted | In allowlist |
|---|---|---|---|---|
| `ThreadStarted` | A thread was created in `composing` state (debounced first user input on a fresh compose). Carries `mode: String` (initial compose mode), optional `actor`. | lifecycle | yes | no |
| `ThreadDiscarded` | A composing thread was explicitly discarded (DELETE /threads/:id). Terminal — the state-machine guard rejects all subsequent compose mutations with 410 Gone. | lifecycle | yes | no |
| `ThreadTitleGenerated` | The title-generation pass produced a title for the thread (background, after enough body to summarize). | lifecycle | yes | no |
| `ThreadTitleRenamed` | The user manually renamed the thread. | lifecycle | yes | no |
| `ThreadSaved` | User pinned / saved the thread. Empty payload. | lifecycle | yes | no |
| `ThreadUnsaved` | User unsaved. Empty payload. | lifecycle | yes | no |
| `ThreadArchived` | User archived. Empty payload. | lifecycle | yes | no |
| `ImageUploaded` | A user attached an image to a compose draft (POST /api/v1/threads/:id/blobs). Carries `hash` (sha256, sole identity), `mime`, `byte_size`, optional `actor`. Bytes live exactly once at `data/blobs/<hh>/<hash>.<ext>`. | per-action | yes | no |
| `TriggerStarted` | A scheduled or event-driven trigger run started. Carries `trigger_id`, optional `trigger_name`, optional `prompt`, optional `invocation: TriggerInvocation` (`Schedule` or `Event { event_type, event_id }`), optional `origin`, `go_to_review: bool`. Aliases on the wire: `task_id`, `task_name` (legacy from when triggers were called "scheduled tasks"). | lifecycle | yes | no |
| `TriggerCompleted` | A trigger run finished. Carries `trigger_id`, optional `trigger_name`, optional `result_summary`. Same aliases. | lifecycle | yes | no |

## Changes (per-thread CC change proposals)

The change family is per-thread — `change_id` is the primary identifier. `ChangeProposed` is emitted per-commit by a post-commit hook in the CC worktree (Phase 4.2), so multiple events with the same `change_id` can land for one branch (each carrying a unique `commit_sha`). The DB-level `changes` table is a projection over these events.

| Event | When it fires | Volume | Persisted | In allowlist |
|---|---|---|---|---|
| `ChangeProposed` | A CC commit landed in the worktree (per-commit hook). Carries `change_id`, optional `description`, `files`, `requires_restart`, optional `origin`, optional `commit_sha`, `branch_name`, `repo_root`, `hardened`, `incomplete`, plus legacy `path` / `diff` for old rows. `incomplete: true` when the proposing CC turn ended in `ResponseFailed`. | per-action | yes | no |
| `ChangeApplied` | A change was merged to main. Carries `change_id`, `requires_restart`, `client_update`, `commits: Vec<String>` (subjects, oldest first), optional `thread_title`, optional `actor`, optional `pre_merge_sha` / `post_merge_sha` (used by Revert), legacy `path`. | lifecycle | yes | no |
| `ChangeDiscarded` | A pending change was discarded. Carries `change_id`, optional `actor`, legacy `path`. | lifecycle | yes | no |
| `ChangeReverted` | An applied change was reverted. Carries `change_id`, optional `actor`, legacy `path`. | lifecycle | yes | no |
| `ChangeApplyFailed` | Apply attempt failed mid-merge. Carries `change_id`, `error`, optional `actor`. | lifecycle | yes | no |
| `ChangeHardened` | The change's working tree was hardened (`/harden` marker stamped on HEAD). Idempotent — projection treats only the latest event per `change_id`. Implicitly downgraded when a fresh `ChangeProposed` arrives with `hardened: false`. | lifecycle | yes | no |
| `MergeConflictDetected` | Engine detected a merge conflict pulling main into a CC branch. Carries `change_id`, `files`, optional engine-stamped `origin`. | lifecycle (rare) | yes | no |
| `MergeResolutionStarted` | A merge-resolution worktree was set up. Carries `change_id`, `worktree_path`, `temp_branch`. Survives restart so startup cleanup can find dangling worktrees. | lifecycle (rare) | yes | no |
| `MergeResolutionCleared` | The merge-resolution worktree was torn down (cleanup finished). Carries `change_id`. | lifecycle (rare) | yes | no |

## Cross-thread / context

| Event | When it fires | Volume | Persisted | In allowlist |
|---|---|---|---|---|
| `ChildThreadCompleted` | A child thread spawned by `run_thread` / `run_claude` reached a terminal event (CC: `CodingAgentIdled` or `SessionEnded`; chat: `ResponseGenerated` / `ResponseFailed`). Emitted on the **parent** thread by EventBus fan-in. Carries `child_thread_id`, optional `child_thread_title`, `status: ChildCompletionStatus` (`Success` / `Failure` / `NoChanges` / `Canceled`), `summary` (truncated to 2000 chars; indexed by `indexable_text`), `pending_change_ids`. | per-action | yes | no |
| `ContextDismissed` | The agent (LLM) explicitly asked to drop a prior `ToolCalled` / `ToolResult` / `ChildThreadCompleted` from future resume context, via the `dismiss_from_context` tool. Carries `dismissed_event_id`. The resume helper honours it on every subsequent assembly. | per-action | yes | no |
| `WorktreeCleaned` | Background worktree cleanup ran on this thread (Phase 10.2/10.3). Carries `tier: u8` (1 = build artifacts stripped, worktree still on disk; 2 = entire worktree removed), `freed_bytes: u64` (best-effort), `branch_deleted: bool` (Tier 2 also dropped a fully-merged branch). | lifecycle (rare per thread) | yes | no |

## Transient — never persisted, broadcast over SSE only

These cannot trigger (the matcher only sees persisted events). They drive live UI state (streaming, modal opens, in-app refreshes) and parent-thread fan-out signals.

| Event | When it fires | Volume |
|---|---|---|
| `TextStreaming` | One streamed-text chunk from the chat agentic loop. The pre-finalize counterpart of `TextStreamed`. | high-volume-streaming |
| `Retrying` | The chat agentic loop is retrying (rate-limit, transient API error, "retry with different approach" path). Carries `reason: String`. | per-action |
| `PreambleCompleting` | Reserved variant — defined on the enum and skipped by the projection's transient match arm, but **not emitted** by any production code path today. Treat as a stub for future use. | n/a |
| `CredentialRequest` | Side-effect command — opens the credential prompt modal. Pairs with persisted `CredentialRequested`. Carries `payload: String`. | per-action |
| `PluginInstallRequest` | Side-effect command — opens the plugin install panel. Carries the JSON preview emitted by `install_plugin` (manifest, file list, overwrites, optional setup). Resolved by `POST /api/v1/plugins/install/{install_id}/{confirm\|cancel}`. | per-action |
| `PluginUninstallRequest` | Side-effect command — opens the plugin uninstall panel. Carries the JSON preview from `uninstall_plugin` (plugin name + version, file list partitioned into still-on-disk vs already-missing). Resolved by `POST /api/v1/plugins/uninstall/{uninstall_id}/{confirm\|cancel}`. | per-action |
| `EmailConfirmRequest` | Side-effect command — opens the email confirmation modal. Carries `payload: String`. | per-action |
| `PushNotificationRequest` | Side-effect command — prompts the device to register for web push. Empty payload. | lifecycle |
| `McpConsentRequest` | Side-effect command — opens the MCP consent prompt modal. Carries `data: String`. Pairs with persisted `McpConsentRequested`. | per-action |
| `RefreshFile` | Tells the frontend / open editors to re-read a file at `path`. Emitted by `agentic_loop_special_tool`. | per-action |
| `RefreshAppUI` | Tells any open app iframe with `app_id` to reload itself. | per-action |
| `CaptureAppUI` | Asks an open app iframe to capture state for `request_id`. The reply lands via the SDK capture path. | per-action |
| `NavigationRequested` | Tells the frontend to navigate (URL, intra-app route, etc.). Carries `payload: String`. | per-action |
| `CodingAgentThreadSpawned` | A child CC thread (spawned via `run_claude` / `run_thread`) has started. Carries `cc_thread_id`, `title`, `agent`. SSE-only — the persisted record of the child is its own thread row. Alias: `CcThreadSpawned`. | per-action |
| `ChildrenCountChanged` | A parent thread's `(active_children_count, total_children_count)` flipped. Carries `active: i64`, `total: i64`. Drives the "Active children" badge. | per-action |

## Indexable text

`ThreadEvent::indexable_text()` returns `Some(&str)` for variants whose body should be indexed into the memory store: `MessageReceived`, `UserPromptInjected`, `ResponseGenerated`, `ResponseCanceled`, `ResponseAborted`, `ChildThreadCompleted`. All others are `None`.

## Concrete payload shapes (selected)

For `CodingAgentIdled`, `UserQuestionAsked`, `UserQuestionAnswered`, and the `CodingAgentPermission*` pair — see `system-knowhow/coding-agent-events.md`. The shapes below cover the most-asked chat / lifecycle / change variants.

### `MessageReceived`

```json
{
  "type": "MessageReceived",
  "data": {
    "text": "Summarize my open PRs.",
    "user_image_hashes": [],
    "device_id": "device-abc123",
    "device": "Kenneth's MacBook",
    "image_description": null,
    "parent_thread_id": null,
    "spawning_event_id": null,
    "mode": "human",
    "model": "claude-opus-4-7",
    "reasoning_effort": null,
    "origin": {
      "kind": "device",
      "device_id": "device-abc123",
      "label": "Kenneth's MacBook"
    }
  }
}
```

`mode` is `ActorMode` (`human` / `agent` / `engine`). `origin` is the structured `MessageOrigin` (`Device` / `Api` / `Workspace` / `ThreadLink` / `Engine` / `System`). Old DB rows may be missing `origin` — the frontend's `legacyOrigin()` synthesizes from `device_id` / `parent_thread_id`.

### `ResponseGenerated`

```json
{
  "type": "ResponseGenerated",
  "data": {
    "text": "You have 3 open PRs: …",
    "images": [],
    "model": "claude-opus-4-7",
    "reasoning_effort": "medium"
  }
}
```

### `ResponseCanceled` / `ResponseAborted` / `ResponseFailed`

```json
{ "type": "ResponseCanceled", "data": { "text": "partial…", "images": [], "model": "claude-opus-4-7", "reasoning_effort": null, "cause": "user_stop" } }
{ "type": "ResponseAborted",  "data": { "text": "",        "images": [], "model": "claude-opus-4-7", "reasoning_effort": null, "cause": "engine_shutdown" } }
{ "type": "ResponseFailed",   "data": { "error": "upstream 503: model overloaded" } }
```

`cause` values:

- `CancelCause`: `user_stop` (Stop button), `user_action` (Apply / Discard / Archive on running thread), `unknown` (legacy DB rows).
- `AbortCause`: `engine_shutdown`, `safety_net`, `recovery_after_restart`, `process_killed`, `stale_settle`, `unknown` (legacy).

### `BackgroundBashStarted` / `BackgroundBashCompleted`

```json
{
  "type": "BackgroundBashStarted",
  "data": {
    "task_id": "bash-7f2c…",
    "command": "cargo test -p lucidos-engine --lib",
    "timeout_secs": 600,
    "started_at": "2026-05-13T18:23:01Z"
  }
}
```

```json
{
  "type": "BackgroundBashCompleted",
  "data": {
    "task_id": "bash-7f2c…",
    "command": "cargo test -p lucidos-engine --lib",
    "exit_code": 0,
    "stdout": "running 1842 tests …",
    "stderr": "",
    "started_at": "2026-05-13T18:23:01Z",
    "finished_at": "2026-05-13T18:25:47Z",
    "timed_out": false,
    "killed": false
  }
}
```

`exit_code: null` means the watchdog killed the child on timeout (signal-only exit gives no usable code on macOS).

### `ContextCaptured`

```json
{
  "type": "ContextCaptured",
  "data": {
    "producer": "MainLlm",
    "model": "claude-opus-4-7",
    "context_window": 200000,
    "sections": [
      { "name": "system_prompt", "estimated_tokens": 12345, "characters": 49380 },
      { "name": "memory", "estimated_tokens": 423, "characters": 1690 }
    ],
    "tools": ["bash", "read", "edit"],
    "estimated_total_tokens": 28934,
    "usage": { "input_tokens": 28310, "output_tokens": 0, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0 },
    "trimmed": false
  }
}
```

`producer` is `ContextProducer` (e.g. `MainLlm`, `ClaudeCode`). `usage` is `None` pre-call and on providers that don't report it (OpenAI, Gemini); when present it carries the real provider-reported counts.

### `ChangeProposed`

```json
{
  "type": "ChangeProposed",
  "data": {
    "change_id": "chg-2025-05-13-…",
    "description": "Add ThreadEvent reference doc",
    "files": ["system-knowhow/thread-events.md"],
    "requires_restart": false,
    "origin": null,
    "commit_sha": "f6ae7364e…",
    "branch_name": "claude-code/20260513-181832-48a72d",
    "repo_root": "/Users/.../workspaces/dev",
    "hardened": false,
    "incomplete": false,
    "path": "",
    "diff": ""
  }
}
```

Multiple events with the same `change_id` arrive for a branch (one per commit). The projection in `core::changes_projection` folds them into a single `changes` row.

### `ChangeApplied`

```json
{
  "type": "ChangeApplied",
  "data": {
    "change_id": "chg-2025-05-13-…",
    "requires_restart": false,
    "client_update": false,
    "commits": ["docs: add thread-events reference"],
    "thread_title": "Document all ThreadEvents",
    "actor": { "kind": "device", "device_id": "device-abc123", "label": "Kenneth's MacBook" },
    "pre_merge_sha": "9b38db1b4…",
    "post_merge_sha": "a1b2c3d4e…",
    "path": ""
  }
}
```

### `ChildThreadCompleted`

```json
{
  "type": "ChildThreadCompleted",
  "data": {
    "child_thread_id": "550e8400-e29b-41d4-a716-446655440000",
    "child_thread_title": "Sub-task: rename foo to bar",
    "status": "success",
    "summary": "Renamed all 14 occurrences across 9 files. Tests pass.",
    "pending_change_ids": ["chg-2025-05-13-…"]
  }
}
```

`status` is `success` / `failure` / `no_changes` / `canceled`. `summary` is truncated to 2000 chars. `pending_change_ids` is empty for chat children and for CC children that ended without proposing anything.

### `TriggerStarted` / `TriggerCompleted`

```json
{
  "type": "TriggerStarted",
  "data": {
    "trigger_id": "trg-cc-question-push",
    "trigger_name": "Notify on Claude question",
    "prompt": null,
    "invocation": { "kind": "Event", "event_type": "UserQuestionAsked", "event_id": "…" },
    "origin": { "kind": "engine", "reason": { "kind": "scheduler", "trigger_id": "trg-cc-question-push", "trigger_name": "Notify on Claude question" } },
    "go_to_review": false
  }
}
```

```json
{
  "type": "TriggerCompleted",
  "data": {
    "trigger_id": "trg-cc-question-push",
    "trigger_name": "Notify on Claude question",
    "result_summary": "sent push notification"
  }
}
```

`task_id` / `task_name` aliases on the wire are still accepted on read (legacy from when triggers were called "scheduled tasks") but new emissions use `trigger_id` / `trigger_name`.

### `WorktreeCleaned`

```json
{
  "type": "WorktreeCleaned",
  "data": {
    "tier": 2,
    "freed_bytes": 4823104,
    "branch_deleted": true
  }
}
```

`tier: 1` = build artifacts (`target/`, `node_modules/`, `.lucidos/cache/`) stripped from a long-idle worktree (still on disk). `tier: 2` = entire worktree directory removed. `branch_deleted: true` only on Tier 2 sweeps that also dropped a fully-merged branch.

### `ContinuationStarted`

```json
{
  "type": "ContinuationStarted",
  "data": {
    "branch": "claude-code/…",
    "origin": { "kind": "engine", "reason": { "kind": "continuation_started" } }
  }
}
```

`origin.reason.kind` is `continuation_started` (legacy alias `session_recovered`).

## How a workspace would actually trigger on these

Today: only `UserQuestionAsked`. Anything else needs one of:

1. **Add the event to the scheduler allowlist** in `crates/lucidos-engine/src/scheduler/mod.rs` (engine code change). Trade-off: every emission of that event runs through the trigger matcher — fine for `lifecycle` and `one-per-turn` events, never appropriate for `high-volume-streaming` ones.
2. **Have the relevant code path also `emit_event(...)` a domain event** the trigger can listen to. `SystemEvent::DomainEvent` flows through the matcher unconditionally. This is the right move for "I want a workspace-visible signal that doesn't need to be a first-class engine event." See the `emit_event` LLM tool and `lucidos events emit` CLI in `system-knowhow/lucidos-cli.md`.
3. **For trigger failures specifically** — the scheduler already auto-creates an error notification when a trigger run blows up. No extra wiring needed.

Tell the user the cost up front when they ask for "trigger on X" where X isn't `UserQuestionAsked` — the answer is never one line of trigger config.

## Recipe-shaped guidance

For trigger config syntax (cron format, `on_event`, the `condition` operator vocabulary `$eq` / `$ne` / `$lt` / `$lte` / `$gt` / `$gte` / `$in`), see `system-knowhow/building-a-trigger.md`. Conditions are pure payload filters — they read top-level fields of the event payload (the `data: { … }` object above), nothing else.

For the CC slice — the `UserQuestion` vs permission distinction, the exact `CodingAgentIdled` field semantics, and the no-`CodingAgentErrored` gap — see `system-knowhow/coding-agent-events.md`.

For event-store column shape (`event_type`, `payload`, `created`, `aggregate`, `aggregate_id`, `sequence`) and the queries used to walk threads from events, see `.claude/rules/db.md`.
