---
name: ThreadEvent Reference
description: The full enumerated list of `ThreadEvent` variants the Lucidos engine emits — chat-side (MessageReceived, ResponseGenerated, …), coding-agent (CodingAgent*), thread lifecycle (ThreadStarted, ThreadArchived, …), changes (ChangeProposed, ChangeApplied, …), background bash, plugin / repo / merge-conflict, transient SSE-only request events (AppUiRefreshRequested, AppUiCaptureRequested, …). Documents each variant's payload, volume class, whether it's persisted, and whether a workspace trigger can subscribe to it via an `on:` entry (the scheduler uses a blocklist — every persisted variant is triggerable except a small set of high-volume streaming / per-action ones). Load when the user asks "what events fire on a thread", "can I trigger on ResponseGenerated / ChangeApplied / TriggerCompleted / BackgroundBashCompleted", "list every ThreadEvent", "is X event persisted", "what payload does Y carry", or talks about the EventBus, event sourcing, SSE event stream, event-based triggers, projections — and especially before wiring an `on:` subscription to a high-volume variant. For the CC-only deep-dive (UserQuestion vs permission distinction, CodingAgentIdled semantics, the no-`CodingAgentErrored` gap), see `system-knowhow/coding-agent-events.md`.
---

# ThreadEvent Reference

The complete enumerated list of `ThreadEvent` — the per-thread event family that flows through the EventBus into PostgreSQL, the SSE stream, and (for one curated entry) the trigger matcher. Source of truth: `crates/lucidos-engine/src/engine/thread_events.rs`. Variant names below are the **current** names; legacy aliases (e.g. `ClaudeCodeIdled`, `SessionRecovered`, `parent_thread`, `task_id` / `task_name`) exist as `#[serde(alias = ...)]` on the wire so old DB rows decode cleanly — write new code, new triggers, and new docs against the current name only.

For the CC slice (`CodingAgent*` + the `UserQuestion*` / permission machinery) the deep-dive lives in `system-knowhow/coding-agent-events.md`. This file is the master enumeration; the CC entries below summarize and link.

For event-store column shape, the chat-mode terminator set, and the `events` table schema, see `.claude/rules/db.md`. For trigger config syntax (cron, the `on` subscription list, per-entry `condition` operators), see `system-knowhow/building-a-trigger.md`.

## Today the scheduler uses a blocklist

The scheduler subscribes to the EventBus and forwards events to the trigger matcher. The `BusEvent::Thread` branch is gated by a small **blocklist** (`ThreadEvent::is_per_token_streaming` in `crates/lucidos-engine/src/engine/thread_events.rs`; the gate itself lives in `crates/lucidos-engine/src/scheduler/mod.rs`). Every other persisted `ThreadEvent` is forwarded to the matcher and can be subscribed to via an `on:` entry on a trigger.

The blocklist contains exactly the per-token streaming variants — many fires per turn (one event per text chunk), never appropriate to subscribe a trigger to:

- `TextStreamed`
- `ThoughtStreamed`
- `CodingAgentTextStreamed`

Per-action variants with high cardinality (`ToolCalled`, `ToolResult`, `CodingAgentToolCalled`, `CodingAgentToolResult`, `ContextCaptured`, `MemorySearched`, `ImageDescribed`, `UserPromptInjected`, `CodingAgentPromptSent`) are **triggerable** — fire once per discrete action, scope with per-entry `condition:` filters (e.g. `name: "Bash"`, `args.command: { $regex: "git push" }`, `estimated_total_tokens: { $gt: 150000 }`).

That means right now (each example below is one entry inside a trigger's `on` list — see `system-knowhow/building-a-trigger.md` for the full subscription shape):

- `event_type: UserQuestionAsked` — works. The typical use is "push me when an interactive question is raised so I can answer from my phone." Pair `send_notification` with `tap: { kind: 'navigate', to: { target: 'thread', id: '<thread_id>', event_id: '<source_event_id>' } }` so the tap deep-links straight to the question — see `building-a-trigger.md` for the worked example.
- `event_type: CodingAgentPermissionRequest` / `CommandPermissionRequested` / `CredentialRequested` / `McpConsentRequested` — **work**. These are the other blocking-request events that should wake the user. `CommandPermissionRequested` is the chat command-guard card (ADR 0002).
- `event_type: ResponseGenerated` / `ResponseFailed` / `CodingAgentIdled` / `ChangeApplied` / `ChangeHardened` / `TriggerCompleted` / `BackgroundBashCompleted` / every `Change*` / every `Thread*` lifecycle event — **work**.
- `event_type: ToolCalled` / `CodingAgentToolCalled` / `ContextCaptured` / `ImageDescribed` etc. — **work**. Use a per-entry `condition:` filter to scope; without one a chatty per-action variant will fire the trigger many times per turn.
- `event_type: TextStreamed` / `ThoughtStreamed` / `CodingAgentTextStreamed` — does not fire. The trigger config will validate and the trigger row will be persisted, but the matcher never sees the event. Don't subscribe to per-token streaming variants — they'd saturate whatever they're wired to.
- `event_type: <a workspace-emitted DomainEvent>` — works. `SystemEvent::DomainEvent` (emitted via `lucidos events emit` / the `emit_event` LLM tool) flows through the matcher unconditionally. This is the supported path for "trigger on something my workspace observes."

If you add a new per-token streaming variant to `ThreadEvent`, add it to `ThreadEvent::is_per_token_streaming` in the same change. Lifecycle / one-per-turn / per-action variants need no scheduler change — they flow through by default.

The `Triggerable` column on every table below is the binary "would a trigger fire on this today?" answer. **Triggerable does not mean "good idea to subscribe without a condition"** — for any per-action variant, lean on `condition:` filters to scope the matches.

## Persisted vs transient

The enum splits into two halves, mirrored by `ThreadEvent::is_persisted()`:

All variants are past tense — events-only model, no command concept (imperative actions are reframed as request events like `AppUiRefreshRequested`). Persistence is orthogonal to tense:

- **Persisted.** `MessageReceived`, `ResponseGenerated`, `CodingAgentIdled`, `ChangeApplied`, etc. Written to the `events` table; replayable; visible to projections, history queries, and (in principle) the trigger matcher.
- **Transient.** `CumulativeTextUpdated`, `LlmCallRetried`, `AppUiRefreshRequested`, `PluginInstallRequested`, etc. Broadcast over SSE only. Never persisted; never reach the projection or trigger paths. Used for live UI updates (token streaming preview, modal-trigger request events) and child thread broadcasts.

A trigger on a transient event can never fire — the scheduler's matcher only looks at persisted events.

## Wire format and metadata

Persisted events are stored with `event_type` set to the variant name and `payload` as the variant's JSON object. Cross-cutting fields merged into the payload at persist time by `EventMeta` (see `engine/thread_events.rs` `EventMeta::apply`):

- `request_event_id` — links response/terminal events back to the originating request.
- `channel` — `"chat"` / `"claude_code"` / `"trigger"` (`EventChannel`). The `"claude_code"` wire string is the deliberate Claude-Code instance identifier (not a legacy alias) — a future Codex coding agent would slot in as `EventChannel::Codex` with wire string `"codex"`.
- `actor` — `MessageOrigin` of who initiated. Stamped by mutating HTTP handlers via `api/actor::user_actor_resolved`.

Some variants (`ChangeApplied`, `ChangeDiscarded`, `ChangeReverted`, `ChangeApplyFailed`, `ChangeHardened`, `ThreadStarted`, `ThreadDiscarded`, `ImageUploaded`) carry `actor` as a per-variant field (predates `EventMeta`); `MessageReceived` and several others use `origin: Option<MessageOrigin>`. Treat both as the canonical "who did this" field for that event.

## Volume classes

Used in every table below. Pick the right class before subscribing a trigger.

- **lifecycle** — fires once at a moment in a thread's life (creation, archive, terminal). Safe to trigger on directly.
- **one-per-turn** — fires at most once per chat / CC turn. Safe to trigger on, with a `condition` if needed.
- **per-action** — fires once per discrete user/agent action (one tool call, one message, one captured context). Triggerable, but always pair with a `condition` filter — without one, a chatty per-action variant will fire the trigger many times per turn.
- **high-volume-streaming** — many fires per turn (per token chunk). **Blocked by the scheduler** (`TextStreamed`, `ThoughtStreamed`, `CodingAgentTextStreamed`); the matcher never sees them. Subscribing is a no-op.
- **transient-SSE-only** — never persisted; cannot trigger.

## Chat / agentic loop

These fire on chat threads (`channel = chat`) and on trigger-driven runs (`channel = trigger`). The CC equivalents under "Coding agent" use the parallel `CodingAgent*` names.

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `MessageReceived` | A user (or upstream workspace, or parent thread, or engine) submitted text into the thread. Stamped at the HTTP boundary in `api/chat.rs::chat_submit`. | one-per-turn | yes | yes |
| `TextStreamed` | A complete chunk of assistant text was committed to the thread (post-stream finalize for chat; one event per appended chunk). | high-volume-streaming | yes | **no (blocked)** |
| `ThoughtStreamed` | The model emitted a `thinking` / reasoning block (extended-thinking models). One event per chunk. Legacy alias: `Thinking`. | high-volume-streaming | yes | **no (blocked)** |
| `ContextCaptured` | One snapshot of the LLM context the engine assembled for a single LLM call (prompt sections, tools, estimated tokens, real `usage` when the provider reports it). One per LLM call; the modal reads these to show context drift. | per-action | yes | yes (use condition) |
| `MemorySearched` | The chat-side memory consumer ran a vector search to assemble context. Carries `results: usize`, `queries: Vec<String>`. | per-action | yes | yes (use condition) |
| `ToolCalled` | The chat agentic loop invoked a tool (`name`, `args`, optional `description`). Distinct from `CodingAgentToolCalled` — those are CC's tool calls. | per-action | yes | yes (use condition) |
| `ToolResult` | The result returned to the chat agentic loop for a prior `ToolCalled`. Carries `result: String`, `images`, `success: bool` (default true), and optional `tool_called_event_id: Uuid` set only by the post-restart recovery sweep (`recover_orphan_tool_calls`) for synthetic backfills — the frontend's `groupIntoExchanges` uses it to land the synthetic result in the same exchange as its orphan `ToolCalled` so the "Executing …" spinner resolves. Live emits omit it; chronological name pairing handles those. | per-action | yes | yes (use condition) |
| `BackgroundBashStarted` | A long-running task was spawned via `run_bash_background` (shell command) OR `run_python_background` (venv-rooted Python script — the engine wraps it as `/bin/sh -c "<venv-python> <script>"` and routes it through the same registry). The `command` field captures the exact shell invocation. Paired with a later `BackgroundBashCompleted`. | per-action | yes | yes |
| `BackgroundBashCompleted` | The task finished — natural exit, watchdog timeout, or `bash_kill`. Carries `exit_code: Option<i32>` (None on signal-only kills), `stdout`, `stderr`, `timed_out: bool`, `killed: bool`. The audit-trail counterpart of `Started`; `bash_output` falls back to this row after the in-memory registry evicts the task. Same shape whether the spawning tool was `run_bash_background` or `run_python_background`. | per-action | yes | yes |
| `ResponseGenerated` | The chat agentic loop terminated with an assistant response. The chat-mode terminator. Carries `text` (`#[serde(skip_serializing_if = "is_empty_str")]`), `images`, `model`, `reasoning_effort`. **`text` may be empty**: when a turn ends on a clean, model-decided stop with no text and no tool calls (a *benign empty completion* — e.g. Gemini `finishReason: STOP` after successful tool calls), the loop emits an empty `ResponseGenerated` rather than `ResponseFailed`, so the thread completes Idle instead of showing a red error. The UI renders a neutral "model returned an empty response" note for an empty-bodied completion. See `classify_empty_completion` (`agentic_loop/helpers.rs`) for the benign-vs-failure split. | one-per-turn | yes | yes |
| `ResponseCanceled` | User clicked Cancel, or clicked Apply / Discard / Archive on a still-running session. Carries `cause: CancelCause` (`UserStop` / `UserAction` / `Unknown`). Always emit via `thread_events::emit_response_canceled` — it's idempotent against pre-emitted terminators (the `/api/v1/restart` race). | one-per-turn | yes | yes |
| `ResponseAborted` | System-driven termination — engine shutdown, safety net (non-watchdog), recovery sweep, OS signal, stale-projection settle. Carries `cause: AbortCause` (`EngineShutdown` / `SafetyNet` / `RecoveryAfterRestart` / `ProcessKilled` / `StaleSettle` / `Unknown`). Always emit via `thread_events::emit_response_aborted`. Note: when a hung-subprocess watchdog kills CC (vs a CC crash or driver death), the engine emits `ContinuationRequested{auto_recovery_after_hang}` instead of `ResponseAborted{SafetyNet}` so the thread auto-resumes without user intervention. Two watchdogs can fire that path — see the `ContinuationRequested` row below. | one-per-turn | yes | yes |
| `ResponseFailed` | Hard failure mid-turn: upstream API error, panic, OOM-killed bash, empty assistant text on a non-cancel turn (`agent_session::lifecycle::classify_result` triggers this for CC too). Carries `error: String`. For an empty chat completion, `ResponseFailed` is reserved for the *genuine* failure shapes — output **truncated** (`max_tokens` / `MAX_TOKENS` / `length`), **blocked** by a safety/policy classifier (`refusal` / `SAFETY` / `content_filter`), **dropped output** (provider billed tokens but nothing parsed; Anthropic-only signal), or an **unrecognised** stop reason (fail-safe). A clean model-decided empty stop is benign and emits an empty `ResponseGenerated` instead (see that row). Classification is uniform across providers and thread types — see `classify_empty_completion` / `normalize_finish_reason` (`agentic_loop/helpers.rs`). | one-per-turn | yes | yes |
| `UserPromptInjected` | A user correction OR an engine-injected mid-flight message (resume note, child-thread callback in legacy paths) was relayed into the live agentic loop. Carries `text`, `mode: ActorMode`, optional `origin`, optional `injected_message_id`. | per-action | yes | yes (use condition) |
| `ImageDescribed` | A background Flash call produced a text description for one of the images attached to a `MessageReceived`. Emitted from the agentic loop after iteration 1 of a chat turn — one event per attached `user_image_hashes` entry, all carrying the same description text. Replaced an in-place `jsonb_set` mutation that used to write `image_description` back into the source row. Carries `source_event_id: Uuid` (the originating `MessageReceived`), `hash: String` (the described blob's sha256), `description: String` (post `is_bad_image_description` filter), `model: String` (literal `"backfill"` on rows produced by the startup backfill, otherwise the actual Flash model). The `description` is indexed into memory (it carries real shared content — screenshots, tickets, photos), so an image-only turn isn't a memory black hole. | per-action | yes | yes (use condition) |
| `TodoListWritten` | The *Lucidos Agent* called the `todo_write` LLM tool, OR the engine's `todo_consumer` flipped abandoned items at response termination. Replace-whole-list semantics — `items: Vec<TodoItem>` is the new complete *todo list*, fully superseding any prior `TodoListWritten` in the thread. Each `TodoItem` has `content: String` (imperative form, "Run tests"), `active_form: String` (present continuous, "Running tests"), `status: TodoStatus` (`pending` / `in_progress` / `completed` / `abandoned`, snake_case on the wire). LLM tool handler enforces ≤ 50 items, at most one `in_progress`, and rejects `abandoned` (engine-only); empty list is valid and means "cleared". The engine-side `todo_consumer` subscribes to `ResponseGenerated` / `ResponseCanceled` / `ResponseAborted` and re-emits the latest list with any `pending` / `in_progress` items flipped to `abandoned`, so the panel always shows an honest final state once a response ends. Chat-agent tool only — *coding-agent threads* use *Claude Code*'s own `TodoWrite` rendering instead. UI: frontend walks the thread's events backwards, finds the most recent `TodoListWritten`, and renders the items in the prompt-bar collapsible panel; abandoned rows render with a dashed strike-through and an `abandoned` tag. | per-action | yes | yes (use condition) |

Terminator set for chat-mode (`TERMINATOR_EVENT_TYPES` constant): `ResponseGenerated`, `ResponseCanceled`, `ResponseAborted`, `ResponseFailed`. Used by `has_terminator_for` for idempotent terminator emission.

## Resume / continuation

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `ContinuationStarted` | Resume-after-abort boundary. Opens a new exchange in the timeline whose body is the rerun (chat: re-LLM call after abort; CC: `--resume` into the same `cc_session_id`). Carries optional `branch` and engine-stamped `origin`. Aliases for old DB rows: `SessionRecovered`, `SessionResumed`. | lifecycle | yes | yes |
| `SessionStarted` | A coding-agent process spawned. Carries `session_id` (CC's CLI session id), optional `branch`, optional `repo_id`, plus *coding-agent-thread* discriminators: `coding_agent_kind` (`"lucidos" \| "app" \| "external"`, default `"lucidos"`), `coding_agent_folder` (canonical folder the spawn targets — `<ws>/data/apps/<id>/` for App, repo root otherwise), `app_id` (set only for App), and `coding_agent` (`"claude-code" \| "codex"`, default `"claude-code"` — which backend drives the thread; locked in by the first SessionStarted via the `thread_summaries.coding_agent` projection). Legacy rows without these decode as Lucidos / Claude Code via the serde defaults. | lifecycle | yes | yes |
| `SessionEnded` | A coding-agent thread is truly done (Phase 4 of the CC resume architecture: terminal-only). Carries `reason: SessionEndReason` (`Shutdown` / `Panic` / `Closed` / `StaleResume` / `LegacyNonTerminal`). `StaleResume` is the one transient case — the chat handler retries internally; frontend skips the AbortPanel. | lifecycle | yes | yes |

## Coding agent (CC / Codex)

The umbrella `CodingAgent*` family covers Claude Code and Codex (the variants carry `coding_agent: CodingAgent`, default `ClaudeCode`; the wire field has `#[serde(alias = "agent")]` so legacy DB rows persisted before the rename still decode). Each variant has a `#[serde(alias = "ClaudeCode<X>")]` for the legacy pre-rename variant name — write new code against the `CodingAgent*` form.

**See `system-knowhow/coding-agent-events.md` for full payload shapes, the `CodingAgentIdled` field-by-field reference, the `UserQuestion` vs `CodingAgentPermission` distinction, and the no-`CodingAgentErrored` gap.** This table is the index.

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `CodingAgentUserMessageSent` | A user message was relayed into the agent's input stream. | one-per-turn | yes | yes |
| `CodingAgentPromptSent` | An engine-synthesized prompt was injected (orphan-recovery, hardening retrigger, merge-conflict explainer, post-question continuation). Carries `origin: Option<MessageOrigin>`. Audit-only, not rendered in chat. | per-action | yes | yes (use condition) |
| `CodingAgentTextStreamed` | One chunk of CC's assistant text. | high-volume-streaming | yes | **no (blocked)** |
| `CodingAgentToolCalled` | One CC tool invocation. Carries `name`, `args`, optional `description`, `tool_use_id`. | per-action | yes | yes (use condition) |
| `CodingAgentToolResult` | The result returned to CC for a prior `CodingAgentToolCalled`. Same `tool_use_id`. | per-action | yes | yes (use condition) |
| `CodingAgentIdled` | **The CC turn-boundary marker.** Emitted at the end of every CC turn whose Result wasn't an engine-shutdown abort. Carries `has_changes`, `is_external_repo`, `requires_restart`, `cc_session_id`, `coding_agent`, optional `reason`, optional `worktree_path`, optional `worktree_head_sha`, `bg_bash_pending` (recorded-history flag: true when the turn idled with a chat-agent `run_bash_background` task still running; **no longer gates proposal or drives any UI** — the change proposes at idle regardless of background bash, and correctness is covered by harden-at-apply). | one-per-turn | yes | yes |
| `CodingAgentSettingsChanged` | User changed model, reasoning effort, or permission mode mid-session — and also emitted once at CC `Init` carrying `cc_session_id` so the session id is durable before the first `CodingAgentIdled` (lets a mid-turn engine restart still `--resume`). | lifecycle (rare) | yes | yes |
| `CodingAgentPermissionRequest` | A coding agent asked to confirm a tool call. Two raise paths: CC's MCP permission-prompt subprocess (path outside cwd, `.claude/`, `.git/`) and the Codex app-server approval bridge (sandbox-escaping `command_execution` / out-of-worktree `file_change` under `approvalPolicy: on-request`; the exec escape-hatch protocol emits none). | per-action | yes | yes |
| `CodingAgentPermissionResolved` | The above request was answered (or auto-resolved by recovery, or by supersession when the user types a new message instead of clicking — `allowed: false`, `reason: "Superseded by a new message"`). Carries `allowed`, optional `reason`, optional `persist_scope` (`narrow` / `broad` / `session`). | per-action | yes | yes |
| `MissingHardeningDetected` | Engine detected a CC session ended without running `/harden` and auto-spawned a recovery hardening session. Not a session terminator — the thread stays active until hardening finishes. | lifecycle (rare) | yes | yes |
| `ContinuationRequested` | Continuation marker — emitted when an interrupted CC turn needs to resume without a new user message. Picked up by the spawn dispatcher; the event id is the spawn idempotency key. `reason: String` is one of: `"user_clicked_continue"` (user clicked Continue after an engine restart), `"answered_after_idle"` (user answered an `AskUserQuestion` after CC's subprocess was torn down at idle), `"auto_recovery_after_hang"` (a hung-subprocess watchdog detected CC silent past its inactivity limit and auto-resumes without user intervention). Two watchdogs can produce `auto_recovery_after_hang`: the in-loop one inside `run_session`'s `select!` (10 min, fast first line) and an external scanner task (12 min, ticks every 30 s from outside any per-thread loop — catches the case where the `select!` itself is wedged in an event-handler await). The two share a gate, the 2-min grace ensures the in-loop fires first when it can. Past name `ContinueSignal` is kept as a serde alias for old DB rows. | lifecycle | yes | yes |

## Question / permission machinery

Not prefixed `CodingAgent*` because the same machinery serves any agent that needs to ask the user a structured question. All the blocking-request events below are triggerable — wire a trigger to any of them to push the user when the agent needs an answer. See `building-a-trigger.md` for the deep-link pattern (`tap: { kind: 'navigate', to: { target: 'thread', id, event_id } }`).

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `UserQuestionAsked` | An interactive question raised — by CC's `AskUserQuestion` tool, Codex's `ask_user_question` MCP tool (one question per call), or the chat agent's `ask_user_question` tool. `meta.channel` (`claude_code` — the coding-agent channel, both backends — / `chat`) records which lane raised it. Resume happens via `POST /api/v1/threads/{thread_id}/answer-question`; the engine branches on the channel to fire the right resume side-effects (coding-agent: resume marker + `ContinuationRequested` respawn if needed; chat: wake the in-process tool). Carries `tool_use_id`, `cc_session_id` (empty for chat-channel and Codex rows), `question`, `options: Vec<QuestionOption>`, optional `worktree_path`, `multi_select: bool`. | one-per-turn (of the `Asked` kind) | yes | yes |
| `UserQuestionAnswered` | The user (or, on the orphan-recovery path, the engine) supplied an answer. Pairs 1:1 with the matching `UserQuestionAsked` via `tool_use_id`. Carries `answer: AnswerKind` (`Selected` / `FreeText` / `MultiSelected` / `Canceled`) and propagates `meta.channel` from the originating `Asked`. | one-per-turn | yes | yes |
| `CommandPermissionRequested` | The **command guard** (ADR 0002) paused a chat bash/python tool call to ask the user — the `IrreversibleDanger` lane (a likely real-world side-effect — mutating HTTP, sending mail, a cloud-CLI mutation — or destruction outside the workspace). For the ambiguous middle the lane is decided by the LLM **judge** (Phase 3), after a static fast-path settles the obvious safe/catastrophic cases. The chat mirror of `CodingAgentPermissionRequest`: it renders the same `PermissionCard`, but the agent loop blocks in-process (no MCP subprocess). Carries `request_id`, `tool_use_id`, `tool_name` (a bash/python tool), `command` (the inspected text), `summary` (the card's one-line risk, written by the judge). Chat-channel only; flips the thread to `waiting_for_user_answer`. | per-action (only when the guard is on AND a command hits the danger lane) | yes | yes |
| `CommandPermissionResolved` | The above was answered (Allow once / Deny / Allow for this thread / Always allow), or auto-resolved by the engine (`reason: "Superseded by a new message"` when the user types instead of clicking; an orphan/cancel reason on restart or Stop). Carries `request_id`, `allowed`, optional `reason`, optional `persist_scope` (`narrow` / `broad` / `session`). Flips the thread back to `running`. | per-action | yes | yes |
| `CommandCheckpointed` | The **command guard** (ADR 0002, Phase 4) snapshotted the workspace's git-tracked `data/` on a safety ref before running a `ReversibleDanger` command (in-workspace deletion/overwrite) — so the user can one-click Undo. Emitted only after the snapshot succeeds; a failed snapshot runs the command unguarded with no event. Carries `checkpoint_id` (the ref key), `command` (the inspected text), `summary` (the card line). Does not change thread status (it's taken mid-turn, right before the command). | per-action (only when the guard is on AND a command hits the reversible lane) | yes | yes |
| `CommandCheckpointReverted` | The user clicked Undo on a `CommandCheckpointed` card (or the engine resolved it): the workspace was restored from the checkpoint ref and the ref deleted. Carries `checkpoint_id`; stamped with the original turn's `request_event_id` so it groups into the same exchange as its checkpoint (the card renders reverted). | per-action | yes | yes |
| `CredentialRequested` | Persisted audit-log entry: a credential prompt was opened for `provider`. Pairs with the transient `CredentialPromptRequested` SSE request that carries the JSON payload for the modal. | lifecycle | yes | yes |
| `McpConsentRequested` | Persisted audit-log entry: an MCP consent prompt was opened for tool with `args`. Pairs with the transient `McpConsentPromptRequested` SSE request that carries the JSON payload for the modal. | lifecycle | yes | yes |

`QUESTION_OVERTAKEN_EVENT_TYPES` constant — the unified set of event names that mean a `UserQuestionAsked` is no longer the latest interactive point on the thread. Once any of these lands after a question, the next typed user text starts a fresh follow-up rather than a `FreeText` answer. Two categories: **terminal** (`ResponseAborted`, `ResponseCanceled`, `ResponseFailed`, `CodingAgentIdled`); **agent progression** — CC (`CodingAgentTextStreamed`, `CodingAgentToolCalled`, `CodingAgentToolResult`, `CodingAgentPromptSent`) and chat (`TextStreamed`, `ThoughtStreamed`, `ToolCalled`, `ToolResult`). The CC progression category defends against the parallel-tool-call race: CC can emit `AskUserQuestion` alongside sibling tool_uses in one assistant message, the hook blocks only AskUserQuestion, the siblings dispatch and emit events while the question stays unanswered. Without filtering on those events, the user's next typed comment is silently absorbed as a `FreeText` answer to the dead question.

The FreeText fast-path is additionally gated to **human-authored** follow-ups (`ActorMode::Human`). Only a real person typing answers an open question; agent- and engine-driven re-entries on the same thread are not the user's answer and must fall through. The case that motivated this: a **child-thread completion** wakes the parent via `notify_parent_of_child_completion` with `ActorMode::Agent`, feeding a `[CHILD THREAD COMPLETED] …` block through the same chat-turn entry point. Before the guard, that block was consumed as a bogus `UserQuestionAnswered { FreeText }` (actor = `thread_link`/`child`), silently killing the user's open question. Now the wake falls through to the injection fast-path (queued as `WakeFromChild`), so the question stays live for the user and the child's result is processed right after they answer it.

## Thread lifecycle

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `ThreadStarted` | A thread was created in `composing` state (debounced first user input on a fresh compose). Carries `mode: String` (initial compose mode), optional `actor`. | lifecycle | yes | yes |
| `ThreadDiscarded` | A composing thread was explicitly discarded (DELETE /threads/:id). Terminal — the state-machine guard rejects all subsequent compose mutations with 410 Gone. | lifecycle | yes | yes |
| `ThreadTitleGenerated` | The title-generation pass produced a title for the thread (background, after enough body to summarize). | lifecycle | yes | yes |
| `ThreadTitleRenamed` | The user manually renamed the thread. | lifecycle | yes | yes |
| `ThreadSaved` | User pinned / saved the thread. Empty payload. | lifecycle | yes | yes |
| `ThreadUnsaved` | User unsaved. Empty payload. | lifecycle | yes | yes |
| `ThreadArchived` | User archived. Empty payload. | lifecycle | yes | yes |
| `ImageUploaded` | A user attached an image to a compose draft (POST /api/v1/threads/:id/blobs). Carries `hash` (sha256, sole identity), `mime`, `byte_size`, optional `actor`. Bytes live exactly once at `data/blobs/<hh>/<hash>.<ext>`. | per-action | yes | yes |
| `TriggerStarted` | A scheduled or event-driven trigger run started. Carries `trigger_id`, optional `trigger_name`, optional `prompt`, optional `invocation: TriggerInvocation` (`Schedule` or `Event { event_type, event_id?, thread_id? }` — `thread_id` set only for thread-scoped source events; exposed to script triggers as `TRIGGER_EVENT_THREAD_ID`), optional `origin`, `go_to_review: bool`. Aliases on the wire: `task_id`, `task_name` (legacy from when triggers were called "scheduled tasks"). | lifecycle | yes | yes |
| `TriggerCompleted` | A trigger run finished. Carries `trigger_id`, optional `trigger_name`, optional `result_summary`. Same aliases. The engine guarantees `result_summary` is a non-empty, single trimmed line for every run it emits — when a script exits 0 with no stdout it falls back to the script's last non-empty line, else `"<name> completed (exit <code>, no output)"`; an intent run with no final text falls back to `"<name> completed (no output)"`. So a blank summary never surfaces and idle-detector triggers don't read as a no-op fire flood to the learning/audit sweeps. | lifecycle | yes | yes |

## Changes (per-thread CC change proposals)

The change family is per-thread — `change_id` is the primary identifier. `ChangeProposed` is emitted **once per CC turn**, at end-of-turn, gated by `should_propose_change_at_idle` (only fires on `TerminalKind::Generated` with worktree changes; aborts/cancels/failures don't propose). That's the "CC is done with real finished work" contract — the chip never flips on partial mid-turn state.

Legacy: historical events with empty `change_id` + `commit_sha` set are from the old per-commit git hook (deleted along with `commit_hook.rs` + `/api/v1/internal/commit-made` to enforce the "never auto-propose for unfinished work" rule). The projection still handles them on replay, but they're inert — no `thread_summaries` updates, no row inserts into `changes`. The DB-level `changes` table is a projection over the aggregate events only.

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `ChangeProposed` | End-of-turn aggregate emit from `propose_change` (CC finished a turn `Generated` with worktree changes). Carries `change_id` (non-empty UUID), optional `description`, `files`, `requires_restart`, optional `origin`, `commit_sha: None`, `branch_name`, `repo_root`, `hardened`, `incomplete`, plus legacy `path` / `diff` for old rows. `incomplete: true` only on engine-internal recovery paths (orphan recovery, stale-session cleanup) that surface commits whose originating turn was killed before closure; a subsequent clean-Generated turn re-emits with `incomplete: false` and clears the flag. Legacy per-commit shape (empty `change_id`, `commit_sha` set) exists only in historical events and is inert in the projection. | per-action | yes | yes |
| `ChangeApplied` | A change was merged to main. Carries `change_id`, `requires_restart`, `client_update`, `commits: Vec<String>` (subjects, oldest first), optional `thread_title`, optional `actor`, optional `pre_merge_sha` / `post_merge_sha` (used by Revert), legacy `path`. | lifecycle | yes | yes |
| `ChangeDiscarded` | A pending change was discarded. Carries `change_id`, optional `actor`, legacy `path`. | lifecycle | yes | yes |
| `ChangeReverted` | An applied change was reverted. Carries `change_id`, optional `actor`, legacy `path`. | lifecycle | yes | yes |
| `ChangeApplyFailed` | Apply attempt failed mid-merge. Carries `change_id`, `error`, optional `actor`. | lifecycle | yes | yes |
| `ChangeHardened` | The change's working tree was hardened (`/harden` marker stamped on HEAD). Idempotent — projection treats only the latest event per `change_id`. Implicitly downgraded when a fresh `ChangeProposed` arrives with `hardened: false`. | lifecycle | yes | yes |
| `MergeConflictDetected` | Engine detected a merge conflict pulling main into a CC branch. Carries `change_id`, `files`, optional engine-stamped `origin`. | lifecycle (rare) | yes | yes |
| `MergeResolutionStarted` | A merge-resolution worktree was set up. Carries `change_id`, `worktree_path`, `temp_branch`. Survives restart so startup cleanup can find dangling worktrees. | lifecycle (rare) | yes | yes |
| `MergeResolutionCleared` | The merge-resolution worktree was torn down (cleanup finished). Carries `change_id`. | lifecycle (rare) | yes | yes |

## Cross-thread / context

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `ChildThreadCompleted` | A child thread spawned by `run_thread` / `run_claude` reached a terminal event (CC: `CodingAgentIdled` or `SessionEnded`; chat: `ResponseGenerated` / `ResponseFailed`). Emitted on the **parent** thread by EventBus fan-in. Carries `child_thread_id`, optional `child_thread_title`, `status: ChildCompletionStatus` (`Success` / `Failure` / `NoChanges` / `Canceled`), `summary` (truncated to 2000 chars; indexed by `indexable_text`), `pending_change_ids`. | per-action | yes | yes |
| `ContextDismissed` | The agent (LLM) explicitly asked to drop a prior `ToolCalled` / `ToolResult` / `ChildThreadCompleted` from future resume context, via the `dismiss_from_context` tool. Carries `dismissed_event_id`. The resume helper honours it on every subsequent assembly. | per-action | yes | yes |
| `WorktreeCleaned` | Background worktree cleanup ran on this thread (Phase 10.2/10.3). Carries `tier: u8` (0 = applied/clean worktree removed after the short grace; 1 = build artifacts stripped, worktree still on disk; 2 = entire worktree removed — the full-removal tier, also used for *stranded* worktrees whose git admin dir is gone), `freed_bytes: u64` (best-effort), `branch_deleted: bool` (a full removal that also dropped a fully-merged branch; always false for stranded removal). | lifecycle (rare per thread) | yes | yes |

## Transient — never persisted, broadcast over SSE only

All transient names are past tense (events-only model). They cannot trigger (the matcher only sees persisted events). They drive live UI state (streaming preview, modal opens, in-app refreshes) and parent-thread fan-out signals. The "request events" carry the JSON payload that drives a frontend modal; the persisted siblings (`CredentialRequested`, `McpConsentRequested`) are the audit-log entry that the same request opened a prompt.

| Event | When it fires | Volume |
|---|---|---|
| `CumulativeTextUpdated` | One snapshot of the assistant's streaming buffer (cumulative text so far). Emitted at every flush boundary alongside the persisted delta in `TextStreamed`; the frontend just overwrites with the latest snapshot. Legacy alias: `TextStreaming`. | high-volume-streaming |
| `LlmCallRetried` | The chat agentic loop is retrying an LLM call (rate-limit, transient API error, "retry with different approach" path). Carries `reason: String`. Legacy alias: `Retrying`. | per-action |
| `PreambleCompleted` | Reserved variant — defined on the enum and skipped by the projection's transient match arm, but **not emitted** by any production code path today. Treat as a stub for future use. Legacy alias: `PreambleCompleting`. | n/a |
| `CredentialPromptRequested` | Request event — opens the credential prompt modal. Pairs with persisted `CredentialRequested`. Carries `payload: String` (the JSON the modal needs). Legacy alias: `CredentialRequest`. | per-action |
| `PluginInstallRequested` | Request event — opens the plugin install panel. Carries the JSON preview emitted by `install_plugin` (manifest, file list, overwrites, optional setup). Resolved by `POST /api/v1/plugins/install/{install_id}/{confirm\|cancel}`. Legacy alias: `PluginInstallRequest`. | per-action |
| `PluginUninstallRequested` | Request event — opens the plugin uninstall panel. Carries the JSON preview from `uninstall_plugin` (plugin name + version, file list partitioned into still-on-disk vs already-missing). Resolved by `POST /api/v1/plugins/uninstall/{uninstall_id}/{confirm\|cancel}`. Legacy alias: `PluginUninstallRequest`. | per-action |
| `EmailConfirmRequested` | Request event — opens the email confirmation modal. Carries `payload: String`. Legacy alias: `EmailConfirmRequest`. | per-action |
| `PushNotificationRequested` | Request event — prompts the device to register for web push. Empty payload. Legacy alias: `PushNotificationRequest`. | lifecycle |
| `McpConsentPromptRequested` | Request event — opens the MCP consent prompt modal. Carries `payload: String` (field `data` accepted as a serde alias). Pairs with persisted `McpConsentRequested`. Legacy alias: `McpConsentRequest`. | per-action |
| `FileRefreshRequested` | Tells the frontend / open editors to re-read a file at `path`. Emitted by `agentic_loop_special_tool`. Legacy alias: `RefreshFile`. | per-action |
| `AppUiRefreshRequested` | Tells any open app iframe with `app_id` to reload itself. Legacy alias: `RefreshAppUI`. | per-action |
| `AppUiCaptureRequested` | Asks an open app iframe to capture state for `request_id`. The reply lands via the SDK capture path. Legacy alias: `CaptureAppUI`. | per-action |
| `NavigationRequested` | Tells the frontend to navigate (URL, intra-app route, etc.). Carries `payload: String`. | per-action |
| `CodingAgentThreadSpawned` | A child CC thread (spawned via `run_claude` / `run_thread`) has started. Carries `cc_thread_id`, `title`, `agent`. SSE-only — the persisted record of the child is its own thread row. Alias: `CcThreadSpawned`. | per-action |
| `ChildrenCountChanged` | A parent or ancestor thread's aggregate metadata changed. Carries the full updated aggregate (`active_children_count`, `total_children_count`, `blocking_descendant_count`, `attention_descendant_count`, …). Fires when (a) a direct child terminates and the parent's active/total counts shift, or (b) any descendant's "blocking" or "attention-needing" predicate flips (Running, WaitingForUserAnswer, or `has_pending_changes` && CodingAgent — see `is_blocking` / `is_attention_needing`), in which case every ancestor on the chain receives the broadcast with its updated counts. Drives the "Active children" badge, the cascading-archive button-hide (via `blocking_descendant_count`), and the Current-bubble routing in `display_section` (via `attention_descendant_count`). | per-action |

## Indexable text

`ThreadEvent::indexable_text()` returns `Some(&str)` for variants whose body should be indexed into the memory store: `MessageReceived`, `UserPromptInjected`, `ResponseGenerated`, `ResponseCanceled`, `ResponseAborted`, `ChildThreadCompleted`, and `ImageDescribed` (its `description` — the only textual record of an image-only turn). All others are `None`.

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

The `Api` variant carries an optional `source_thread_id`:

```json
"origin": {
  "kind": "api",
  "user_agent": "curl/8.7.1",
  "mode": "agent",
  "source_thread_id": "9c1f-..."
}
```

Set when the engine recognised the request as coming from a Lucidos-spawned subprocess (CC, `run_bash`, `run_python`, scheduled script, `lucidos` CLI). Detection is via the per-engine-startup token: every spawned subprocess gets `LUCIDOS_AGENT_ORIGIN_TOKEN` (and `LUCIDOS_THREAD_ID`) injected into its env; the `lucidos` CLI auto-forwards them as `x-lucidos-agent-origin-token` + `x-lucidos-source-thread-id` headers on every engine call. When the token check passes, mutating HTTP handlers (`apply_change`, `revert_change`, `discard_change`, `chat_submit`, settings writes, …) stamp `Api { mode: "agent", source_thread_id: <spawning thread> }` regardless of what the request body claims — agent actions never appear as "You" cards. External API clients without the token fall through to the regular `Api { mode: "human" }` resolution (no behaviour change for non-subprocess callers). Cross-thread chat injection from a subprocess (target thread ≠ source thread) is refused with 403 at `chat_submit`; see `api::chat::subprocess_chat_legitimate` for the full allow/deny matrix.

`image_description` on this payload is **deprecated** — it survives only on legacy rows persisted before the `ImageDescribed` past-tense event existed. New `MessageReceived` emissions always serialize it as `null` (the field is `Option<String>` with `skip_serializing_if = Option::is_none`). Read the description from `ImageDescribed` instead, joined by `source_event_id`. The startup backfill emits one `ImageDescribed` per legacy `(source, hash)` pair so the new event-based read path covers historical rows too.

### `ImageDescribed`

```json
{
  "type": "ImageDescribed",
  "data": {
    "source_event_id": "550e8400-e29b-41d4-a716-446655440000",
    "hash": "abcd1234...",
    "description": "A screenshot of a calendar invitation showing 'Standup' on March 17, 2026 at 09:00.",
    "model": "claude-haiku-4-5"
  }
}
```

`source_event_id` is the `MessageReceived` this description applies to. `hash` is the sha256 of one attached blob (matches one entry in `MessageReceived.user_image_hashes`). Multi-image messages emit one event per hash — collapse on `source_event_id` to recover the per-message description (every event for the same source carries identical `description` text). `model` is the actual Flash model that produced the text (`claude-haiku-4-5`, `gemini-2.5-flash`, …) — except on rows generated by the one-shot startup backfill, where it's the literal `"backfill"` because the original model identity wasn't recorded.

The `description` is surfaced by `ThreadEvent::indexable_text()` and therefore **indexed into memory** like a message or response — a "what's this?" + screenshot turn would otherwise leave no trace in memory, since the typed `MessageReceived.text` carries none of the image's content. Note CC (coding-agent) threads never emit `ImageDescribed` (the description runs only in the chat agentic loop), so their image turns still index text-only.

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

`text` is omitted on the wire when empty (`skip_serializing_if`). An empty
`ResponseGenerated` is a **benign empty completion**: the model ended its turn
on a clean, model-decided stop (`end_turn` / Gemini `STOP` / OpenAI `stop` /
`completed`) with no text and no tool calls. The chat loop emits this — instead
of `ResponseFailed` — so the thread completes Idle (no red error); the frontend
renders a neutral "the model returned an empty response" note. The genuine
failure shapes (truncation, safety block, dropped output, unrecognised stop)
still emit `ResponseFailed`. The split is provider-agnostic — see
`classify_empty_completion` / `normalize_finish_reason` in
`crates/lucidos-engine/src/engine/agentic_loop/helpers.rs`.

### `ResponseCanceled` / `ResponseAborted` / `ResponseFailed`

```json
{ "type": "ResponseCanceled", "data": { "text": "partial…", "images": [], "model": "claude-opus-4-7", "reasoning_effort": null, "cause": "user_stop" } }
{ "type": "ResponseAborted",  "data": { "text": "",        "images": [], "model": "claude-opus-4-7", "reasoning_effort": null, "cause": "engine_shutdown" } }
{ "type": "ResponseFailed",   "data": { "error": "upstream 503: model overloaded" } }
```

`cause` values:

- `CancelCause`: `user_stop` (Cancel button), `user_action` (Apply / Discard / Archive on running thread), `unknown` (legacy DB rows).
- `AbortCause`: `engine_shutdown`, `safety_net`, `recovery_after_restart`, `process_killed`, `stale_settle`, `unknown` (legacy).

On a *coding-agent thread*, `user_stop` is a **resumable turn boundary**, not a terminator: the `Cancel` button routes through CC's native interrupt (Esc), so the turn is interrupted but the session stays alive — `CodingAgentIdled` (with the `cc_session_id`) follows, the branch is kept, and the next message `--resume`s the same conversation. This is distinct from `user_action` (Apply / Discard / Archive), which DO terminate via their own lifecycle event. See `system-knowhow/coding-agent-events.md` § `CodingAgentIdled` "Cancel = Esc".

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

Both events also fire for `run_python_background`. The `command` field then carries the venv-rooted python invocation, e.g. `'/<ws>/.lucidos/runtime/python/venv/bin/python' '/<ws>/.lucidos/exhaust/<run_id>/script.py'`, so the audit trail records which script ran (the file is preserved under `.lucidos/exhaust/`). One registry, one event pair, one watcher — chat-agent consumers don't branch on the spawning tool.

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

**Snapshot endpoint strips the heavy fields.** `GET /api/v1/threads/:thread_id/events` removes `sections` and `tools` from `ContextCaptured` payloads and stamps `sections_stripped: true` so the events list stays small on heavy threads (one capture can be ~50 kB; a long chat session carries hundreds, totaling many MB the events list never renders). Live SSE emissions still carry the full arrays. To get the stripped pieces back on demand (the step-detail modal does this when the user opens it), call `GET /api/v1/events/:event_id/context` — returns `{ sections, tools }` for that single `ContextCaptured` event. The endpoint is keyed on `event_id` only (UUIDs are unguessable, scope is the same as the snapshot endpoint) so callers don't have to plumb the thread id through their lookup path. For bulk consumers that genuinely need the full payload (e.g. `exportThread.ts` for bug-report dumps), pass `?include_context=true` on the snapshot endpoint instead of N+1 fetches. Triggers and event subscribers don't need to read `sections` / `tools`; if you do, use one of the on-demand paths, not the default snapshot.

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

The result returned to the chat agentic loop for a prior `ToolCalled`. `result` is the textual output (e.g. bash stdout, file contents); `images` carries generated-image hashes that render inline in the chat exchange; `tool_called_event_id` is stamped on every live emit (and on synthetic backfills written by `recover_orphan_tool_calls`) so the frontend's `groupIntoExchanges` pairs the result with its `ToolCalled` exchange via `chatToolCallOwners`. Without explicit pairing, an `ask_user_question` result followed the post-`UserQuestionAsked` request-id redirect into the question divider and left the original exchange's "Executing …" spinner pending forever. Server-side resume-block synthesis (LLM-message pairing) still uses chronological name matching (`collect_tool_pairs_chronological`). The field is optional only for legacy DB rows predating the live-emit stamp.

**Snapshot endpoint strips the heavy field.** Mirrors the `ContextCaptured` contract above: `GET /api/v1/threads/:thread_id/events` removes `result` from `ToolResult` payloads and stamps `result_stripped: true` so the events list stays small on busy CC-style threads (one bash result can be 150 kB+; a long session carries hundreds, totaling ~2 MB the chat exchange never renders — only `StepDetailModal.tsx`'s `<pre class="step-detail-result">` does). Live SSE emissions still carry the full text. The strip keeps `name` + `images` inline — the step row label and generated-image rendering paths in `thread-events.ts` need them. To fetch the dropped text on demand, call `GET /api/v1/events/:event_id/tool-result` — returns `{ result: string | null }` for that single `ToolResult` event (`null` for image-only results, which never had a textual result written). Same event-id-only routing as the context endpoint. The same `?include_context=true` flag on the snapshot endpoint that opts back into `ContextCaptured.sections` now ALSO opts back into `ToolResult.result` — covers `exportThread.ts` and any future bulk consumer. `CodingAgentToolResult` is NOT stripped today; if its bash-output bloat becomes the bottleneck, the same pattern (`strip_*_content` helper + `GET /api/v1/events/:event_id/...` endpoint + a marker field) applies.

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

**At most once per `change_id`.** Every apply path funnels its `ChangeApplied` through `EventBus::emit`, which `FOR UPDATE`-claims the change row and suppresses the emit if the row is already `applied`. So a concurrent double-fire (two applies racing), an HTTP/Apply-All retry, a conflict-recovery cleanup, or a post-restart recovery re-apply can never persist a second `ChangeApplied` for the same change — the timeline shows one "Change applied" entry per change. Recovery no-ops and the external-repo archive carve-out run while the row is still `pending` (or has no `changes` row at all), so they still emit exactly once.

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
    "trigger_id": "trg-question-push",
    "trigger_name": "Push when Claude needs me",
    "prompt": null,
    "invocation": { "kind": "Event", "event_type": "UserQuestionAsked", "event_id": "…", "thread_id": "…" },
    "origin": { "kind": "engine", "reason": { "kind": "scheduler", "trigger_id": "trg-question-push", "trigger_name": "Push when Claude needs me" } },
    "go_to_review": false
  }
}
```

```json
{
  "type": "TriggerCompleted",
  "data": {
    "trigger_id": "trg-question-push",
    "trigger_name": "Push when Claude needs me",
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

`tier: 0` = an applied/clean worktree (branch at main HEAD, no pending change) removed after the short grace window. `tier: 1` = build artifacts (`target/`, `node_modules/`, `.lucidos/cache/`) stripped from a long-idle worktree (still on disk). `tier: 2` = entire worktree directory removed — both the 30-day idle sweep and *stranded* worktrees (their git admin dir under `.git/worktrees/<name>` is gone, so git can't act on them and they're deleted directly). `branch_deleted: true` only on a full removal that also dropped a fully-merged branch; stranded removal never deletes a branch (it touches no refs), so `branch_deleted` is always false there.

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

For any persisted `ThreadEvent` that isn't in the per-token streaming blocklist (i.e. anything other than `TextStreamed` / `ThoughtStreamed` / `CodingAgentTextStreamed`), an `on` subscription works directly:

```yaml
on:
  - event_type: ChangeApplied
    condition:
      hardened: true
run:
  intent: "Tell me which change just landed and what files it touched."
```

Three knobs:

1. **Pick the right event.** Lifecycle / one-per-turn variants are usually what you want. Per-action variants (e.g. `ToolCalled`, `CodingAgentToolCalled`, `ContextCaptured`, `ImageDescribed`) fire many times per turn — always pair them with the entry's `condition:` filter so the trigger only matches the case you care about (e.g. `name: "Bash"`, `args.command: { $regex: "git push" }`, `estimated_total_tokens: { $gt: 150000 }`).
2. **Per-token streaming is off-limits.** `TextStreamed` / `ThoughtStreamed` / `CodingAgentTextStreamed` are blocked at the scheduler. Subscribing to one validates and persists, but the matcher never sees the event. If a workspace genuinely needs token-level reactivity, it has to consume the SSE stream directly — not from a trigger.
3. **For workspace-defined signals**, `lucidos events emit` (or the `emit_event` LLM tool) writes a `SystemEvent::DomainEvent` that flows through the matcher unconditionally. Use this when you want a name that isn't part of the engine's own ThreadEvent enum (e.g. `OuraDataImported`, `BuildBroken`) — see `system-knowhow/lucidos-cli.md`.

Trigger-run failures still auto-create an error notification — no separate wiring needed for "tell me when one of my own triggers blew up."

## Recipe-shaped guidance

For trigger config syntax (cron format, the `on` subscription list, the per-entry `condition` operator vocabulary `$eq` / `$ne` / `$lt` / `$lte` / `$gt` / `$gte` / `$in`), see `system-knowhow/building-a-trigger.md`. Conditions are pure payload filters — they read top-level fields of the event payload (the `data: { … }` object above), nothing else.

For the CC slice — the `UserQuestion` vs permission distinction, the exact `CodingAgentIdled` field semantics, and the no-`CodingAgentErrored` gap — see `system-knowhow/coding-agent-events.md`.

For event-store column shape (`event_type`, `payload`, `created`, `aggregate`, `aggregate_id`, `sequence`) and the queries used to walk threads from events, see `.claude/rules/db.md`.
