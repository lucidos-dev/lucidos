---
name: ThreadEvent Reference
description: Every `ThreadEvent`, by family (chat, coding-agent, lifecycle, changes, background bash, plugin, repo, merge conflict, transient SSE-only). With payload, persistence, and whether a trigger can subscribe. Load for "what events fire on a thread" or "can I trigger on ResponseGenerated / ChangeApplied / BackgroundBashCompleted". Also for "is X persisted" and "is X a real event name".
---

# ThreadEvent Reference

Every `ThreadEvent`: the per-thread event family that flows through the EventBus into PostgreSQL, the SSE stream and the trigger matcher. Source of truth: `crates/lucidos-engine/src/engine/thread_events/` (the enum is in `event.rs`). Names below are the **current** names. Legacy aliases (e.g. `ClaudeCodeIdled`, `SessionRecovered`, `parent_thread`, `task_id` / `task_name`) exist as `#[serde(alias = ...)]` so old DB rows decode. Write new code, triggers and docs against the current name only.

This file is the master enumeration. The coding-agent slice (`CodingAgent*` and the `UserQuestion*` / permission machinery) is summarized here, with the deep-dive in `system-knowhow/coding-agent-events.md`.

Event-store column shape, the chat-mode terminator set and the `events` table schema: `.claude/rules/db.md`. Trigger config syntax (cron, the `on` list, per-entry `condition` operators): `system-knowhow/triggers.md`.

## One table, two enums

**This is the canonical statement of the `ThreadEvent` / `SystemEvent` split. Everything else points here.**

There are two Rust enums and **one** table.

- **`ThreadEvent`** (`crates/lucidos-engine/src/engine/thread_events/`) is the per-thread family in this file: `MessageReceived`, `ResponseGenerated`, `CodingAgentIdled`, `ChildThreadCompleted`, every `Change*`, every `Thread*` lifecycle event.
- **`SystemEvent`** (`crates/lucidos-engine/src/engine/event_bus_system_event.rs`) is the workspace-scoped family: `NotificationCreated`, `TriggerCompleted`, `PluginInstalled`, `PreferencesChanged`, and so on. Its **`SystemEvent::DomainEvent`** variant carries names the workspace invents (`HabitCompleted`, `OuraDataImported`, `LucidosReleased`). Writers: `lucidos events emit`, the `events` LLM tool's `emit` action, `lucidos.events.emit` in an app, or `POST /api/v1/events/emit`.

Both go through the one `EventBus::emit`. Every variant whose `is_persisted()` is true goes through the one `EventBus::persist` INSERT into the one **`events`** table, with `event_type` set to the variant name. A `DomainEvent` is stored under its *inner* type (`HabitCompleted`), never the literal string `"DomainEvent"`.

Only two columns differ:

| | `aggregate` | `aggregate_id` | `thread_id` column |
|---|---|---|---|
| `ThreadEvent` | `'thread'` | the thread id | set (mirrored from `aggregate_id`) |
| `SystemEvent::DomainEvent` | `'domain'` | the event type | NULL |
| other `SystemEvent` | its own (`'notification'`, `'trigger'`, `'plugin'`, `'app'`, …) | the entity id | NULL |

The rest of both enums is **transient**: a `ThreadEvent` whose `is_persisted()` is false (see "Persisted vs transient"), or a `DomainEvent` emitted with `{ transient: true }`. These go out on SSE and are never written, so nobody can query them and they cannot fire a trigger.

**Consequence for reading.** `GET /api/v1/events/query`, `GET /api/v1/events/count`, `GET /api/v1/events/types`, `lucidos events query`, `lucidos.events.query` in an app, and the `events` LLM tool all run one statement over that table with **no aggregate predicate**. They return persisted rows of BOTH enums, filtered by `event_type` and time. All of them leave out the four side-question events (ADR 0320). There is no separate domain-event stream to "switch to".

The case people get wrong: **`ChildThreadCompleted` is queryable by an app today.** It is a persisted `ThreadEvent`, emitted on the **parent** thread by EventBus fan-in. So the row's `thread_id` is the parent, and the payload carries `child_thread_id` / `child_thread_title` / `status` / `summary`. `lucidos.events.query({ event_type: 'ChildThreadCompleted' })` returns it with no engine change.

So the split is a Rust type distinction and a column value, never a storage boundary.

## Today the scheduler uses a blocklist

The scheduler subscribes to the EventBus and forwards events to the trigger matcher. It has one branch per enum, gated differently. The `BusEvent::Thread` branch uses a small **blocklist**, `core::event_subscription::is_subscribable`, called from `crates/lucidos-engine/src/scheduler/mod.rs`. It drops two groups, both defined in `crates/lucidos-engine/src/engine/thread_events/event_impl.rs`:

- **Per-token streaming** (`ThreadEvent::is_per_token_streaming`): `TextStreamed`, `ThoughtStreamed`, `CodingAgentTextStreamed`, `CodingAgentThoughtStreamed`. Each fires once per text chunk, so a trigger never belongs on one.
- **Side questions** (`ThreadEvent::is_side_question_event`): `SideQuestionAsked`, `SideQuestionAnswered`, `SideQuestionFailed`, `SideQuestionDismissed`. They record a card no agent may see, so a subscription on one is refused (ADR 0320).

Every other persisted `ThreadEvent` reaches the matcher and is a valid `on:` entry.

The `BusEvent::System` branch is an **allowlist**: **a persisted `SystemEvent` is subscribable, a transient one is not** (ADR 0113). It admits every variant whose `is_persisted()` is true, plus a `DomainEvent` on either setting of `transient`. So `BackupCompleted`, `BackupFailed`, `NotificationCreated`, `TriggerCompleted`, `PluginInstalled` and the rest of the persisted set are valid `on:` entries. `BackupProgress`, `Toast`, `MemoryRebuildProgress`, `RecoveryProgress`, `EmbeddingModelStatusChanged` and `TreeBackfillProgressed` are transient frames that write no row, so they reach neither matcher. The full persisted list is `SystemEvent::PERSISTED_TYPE_NAMES`, and `.claude/rules/db.md` § Key event types enumerates the variants.

Both fan-outs run one predicate, `core::event_subscription::is_subscribable_system_event`, so a trigger and an `await_event` are offered the same set.

High-cardinality per-action variants (`ToolCalled`, `ToolResult`, `CodingAgentToolCalled`, `CodingAgentToolResult`, `ContextCaptured`, `MemoryRecalled`, `ImageDescribed`, `PromptInjected`, `CodingAgentPromptSent`) are **triggerable**. Scope them with per-entry `condition:` filters (e.g. `name: "Bash"`, `estimated_total_tokens: { $gt: 150000 }`). A condition key is a **field path**, so `{ "args.command": { "$regex": "cargo test" } }` reads one level down. Operators are `$eq` / `$ne` / `$lt` / `$lte` / `$gt` / `$gte` / `$in` / `$nin` / `$regex`, and a bare value means `$eq`. `$or` in key position takes a list of conditions.

**`thread_id` is available on every event in this file, and on none of their payloads.** The engine supplies it at matching time from the event's own thread. So `condition: { thread_id: "<uuid>" }` scopes any event type to one thread: one session's `CodingAgentIdled`, one thread's next `ResponseGenerated`, one thread's next tool call. It never appears in a payload read back with `query_events`, where the row's thread column carries it. A **`SystemEvent`** belongs to no thread, so a domain event or `BackupCompleted` has no `thread_id` to filter on.

What works today (each line is one entry in a trigger's `on` list; full shape in `system-knowhow/triggers.md`):

- `event_type: UserQuestionAsked`: works. Typical use: "push me when a question is raised so I can answer from my phone." Pair `send_notification` with `tap: { kind: 'navigate', to: { target: 'thread', id: '<thread_id>', event_id: '<source_event_id>' } }` to deep-link to the question (worked example in `triggers.md`).
- `event_type: CodingAgentPermissionRequest` / `CommandPermissionRequested` / `McpPermissionRequested`: **work**. The other blocking requests that should wake the user. `CommandPermissionRequested` is the chat command-guard card (ADR 0002); `McpPermissionRequested` is the chat MCP-tool card.
- `event_type: CredentialRequested` / `PluginInstallRequested` / `PluginUninstallRequested` / `EmailConfirmRequested` / `OAuthAuthorizationRequested`: **work**. These are *form requests*. Pair one with `FormRequestResolved` (same `request_id`) to learn how it ended. See § Form requests.
- `event_type: ResponseGenerated` / `ResponseFailed` / `CodingAgentIdled` / `ChangeApplied` / `ChangeHardened` / `TriggerCompleted` / `BackgroundBashCompleted` / every `Change*` / every `Thread*` lifecycle event: **work**.
- `event_type: ToolCalled` / `CodingAgentToolCalled` / `ContextCaptured` / `ImageDescribed` etc.: **work**, scoped with a `condition:`.
- `event_type: TextStreamed` and the other per-token variants: **refused**, at both surfaces.
- `event_type: <a workspace-emitted DomainEvent>`: works. `SystemEvent::DomainEvent` (from `lucidos events emit` / the `emit_event` LLM tool) always reaches the matcher. This is the supported path for "trigger on something my workspace observes."
- `event_type: BackupCompleted` or any other persisted `SystemEvent`: **work**, scoped with the payload's own fields.
- `event_type: BackupProgress` or another transient frame: **refused**, at both surfaces. The refusal names the persisted event that ends the run, e.g. `BackupCompleted` or `BackupFailed`.

**Two more gates apply to every carrier.** First, self-exclusion. Every event a trigger's fire emits carries that trigger's id, and the matcher drops that trigger from the matches. So a trigger never wakes on its own fire, at any depth. Every other subscriber still sees the event.

Second, the recursion cap bounds a chain ACROSS triggers. An event a fire emits dispatches one level deeper than the event that fired it. Past `max_event_trigger_depth` (a *capacity policy* field, default 5) the event is still stored and reaches SSE, but fires no further triggers. An ordinary turn dispatches at depth 0.

**The two gates part company at a spawn.** Work a fire hands off emits UNMARKED, so a sub-thread or coding-agent session it starts still wakes the trigger. That makes "wait for the session I started" work. The depth does reach that work: a sub-thread (`run_thread`), a coding agent (`run_coding_agent`) and a script all run at the fire's OWN depth. A hop is a trigger fire, so a spawn does not buy a fresh chain.

**When the cap stops a fire, you get a notification.** It names the trigger, the event and the ceiling, once per trigger per ten minutes. Raise `max_event_trigger_depth` in the Thread Queue panel if a chain is legitimately longer. `system-knowhow/triggers.md` covers what the two gates mean for authors.

A new per-token streaming variant goes into `ThreadEvent::is_per_token_streaming` in the same change. Other `ThreadEvent` variants need no scheduler change. Neither does a new `SystemEvent`: being persisted is what makes it subscribable.

The `Triggerable` column in every table below answers "would a trigger fire on this today?" **Triggerable does not mean "good idea to subscribe without a condition"**: scope any per-action variant with `condition:`.

### Triggerable is not the same question as awaitable

An *event subscription* comes in **two species**. They share a predicate language, a matcher (`EventSubscription::matches`) and a blocklist, but not their answer for every event:

- A **trigger subscription** is one entry in a *trigger*'s `on:` list, a persistent reactive rule. Each match spawns a NEW thread and leaves the subscription armed. "React to every X."
- A **thread subscription**, internally an *event wait* (`await_event`), belongs to an existing thread. The thread finishes its turn and idles. The first match re-opens THAT thread with the event as a new message, and the subscription is spent. "Continue when the next X happens."

Two questions pick between them, and people forget the first:

1. **Where does the answer go?** A trigger reaches the user as a notification from its own thread. `await_event` re-opens the subscribing thread, so the report lands in the thread they are reading. "Tell me **here** when X happens" is `await_event`, even though it sounds like a standing rule.
2. **How long must it last?** `await_event` is one-shot: you re-arm per event, and re-arming is rate-capped. A reaction that must outlive the conversation is a trigger.

Being blocked is not a precondition: a turn that could end anyway still uses `await_event` when the user wants the next X reported here. But every X forever is a trigger.

The two columns differ for one family, the `EventWait*` events below. They are **triggerable but not awaitable**. A trigger on "a thread's wait timed out" is reasonable, while a wait on `EventWaitStarted` would satisfy itself the instant any thread registers one. The per-token streaming blocklist applies to both, and both validate names the same way (next section).

## Check the name before you subscribe

`EventSubscription::matches` compares event types as **exact strings**. So a typo, a hallucinated name and a retired name all fail alike: the subscription arms, waits and never matches. Both surfaces now refuse that up front.

Every `on:` entry of a trigger, and every entry in `await_event`, is checked at subscription time. Three verdicts:

| The name is… | What happens |
|---|---|
| an engine name, misspelled or retired | **refused**, with the near match named (`CredentialStored` → "Did you mean `CredentialCreated`?") |
| outside the engine's set | **accepted**, with a warning when this workspace has never emitted it |
| a transient frame that really exists | **refused**, pointed at the persisted event that ends the run |

The middle row matters for your own work. "Make X emit an event, then trigger on it" is the ordinary order, so a domain event you have not emitted yet is legitimate. The warning only catches a typo in your own name.

**Look the name up rather than guessing**, with the `events` tool's `event_types` action:

- **`engine`** is the closed set the validator checks against. A name from this list always validates. A name that merely resembles one is refused.
- **`workspace`** is what this workspace's store has seen and the engine does not emit: your own domain events.
- **`retired`** is what the renames took away. **This is the complete list**, read from `ThreadEvent::LEGACY_TYPE_NAME_ALIASES`, which a test holds to the names serde still accepts. The `Legacy alias:` notes in the tables below cover only part of it. Ask the tool; never assemble the set from those notes.

For "has this ever happened, and how often", use the `count` action. With no `event_type` it returns the per-type breakdown, so one call separates a name nobody emits from one that fired twice last year.

**The rename trap.** An event is immutable, so old rows keep the old name. `query_events({ event_type: "ClaudeCodeIdled" })` still returns history, and the name looks alive. Nothing emits it again, so a subscription on it can only match the past. That is why a retired name is refused rather than warned about. Two renames each broke a live subscription: `ClaudeCodeIdled` to `CodingAgentIdled`, and `MemorySearched` to `MemoryRecalled`.

### The path is checked too, not just the name

A `condition` fails the same silent way. `{"version": "0.1.0"}` on `PluginInstalled` is valid and matches nothing: that value sits at `manifest.manifest.version`. So every field path in a condition is checked against the twenty most recent stored payloads of that type. A path in none of them gets a warning, which names the same leaf found deeper down.

It is a warning, never a refusal, because the sample is evidence, not a schema. An optional field can be absent from twenty rows, and `{"conclusion": {"$ne": "success"}}` deliberately matches an event with no `conclusion`. An event type with no stored rows gets no check.

**A condition names the unwrapped payload.** A persisted system event is stored as `{type, data}` and the matcher strips that envelope, so a condition writes `filename`, never `data.filename`. The `events` tool's `query` action returns the stored row, envelope and all. Read a path there, then drop the leading `payload.data.` before writing it into a condition.

## Persisted vs transient

`ThreadEvent::is_persisted()` splits the enum in two. All variants are past tense: there is no command concept, and imperative actions become request events like `AppUiRefreshRequested`. Persistence is independent of tense:

- **Persisted.** `MessageReceived`, `ResponseGenerated`, `CodingAgentIdled`, `ChangeApplied`, etc. Written to the `events` table; replayable; visible to projections, history queries and the trigger matcher.
- **Transient.** `CumulativeTextUpdated`, `LlmCallRetried`, `AppUiRefreshRequested`, `NavigationRequested`, etc. Broadcast over SSE only, and never reach the projection or trigger paths. Used for live UI updates (streaming preview, navigation) and child thread broadcasts. A trigger on one can never fire.

## Wire format and metadata

Persisted events are stored with `event_type` set to the variant name and `payload` as the variant's JSON object. `EventMeta` merges cross-cutting fields into the payload at persist time (`EventMeta::apply` in `crates/lucidos-engine/src/engine/thread_events/meta.rs`):

- `request_event_id`: links response and terminal events back to the originating request.
- `channel`: `"chat"` / `"claude_code"` / `"trigger"` (`EventChannel`). `"claude_code"` is the coding-agent channel for both Claude Code and Codex, kept for compatibility with existing rows and clients.
- `actor`: the `MessageOrigin` of who initiated. Stamped by mutating HTTP handlers via `api/actor::user_actor`.

Some variants (`ChangeApplied`, `ChangeDiscarded`, `ChangeReverted`, `ChangeApplyFailed`, `ChangeHardened`, `ThreadStarted`, `ThreadDiscarded`, `ImageUploaded`) carry `actor` as their own field, which predates `EventMeta`. `MessageReceived` and several others use `origin: Option<MessageOrigin>`. Treat either as that event's canonical "who did this" field.

### Engine origins: `origin.kind == "engine"`

When the engine writes a message or prompt, the event carries `origin: { "kind": "engine", "reason": { "kind": … } }` and `mode: "engine"`. The reason says why. The one exception is an event-wait re-entry anchor, which keeps `mode: "agent"` (§ The re-entry anchor). Two reasons seed a whole thread and name what the engine acted on:

| `reason.kind` | When | Fields |
|---|---|---|
| `plugin_setup` | The first message of a *setup thread*, after the user confirmed an install or update with new setup instructions. | `plugin_id`, `plugin_name`, `version` (just installed), `occasion` (`{ "kind": "fresh_install" }` or `{ "kind": "update", "from_version"? }`), optional `confirmed_on_device_id` |
| `plugin_upstream_proposal` | The first message of a thread that offers the user's local plugin edits to the plugin's author. | `plugin_id`, `plugin_name`, `version`, `patch_path`, optional `confirmed_on_device_id` |

The engine wrote the words, so the engine is the origin, never the user's device. The confirming device rides beside it as `confirmed_on_device_id`, the id only. `from_version` is absent when the prior install record names no version.

The other reasons are `continuation_started`, `orphan_recovery`, `scheduler` (with `trigger_id`, optional `trigger_name`), `harden_retrigger`, `stale_session`, `archived_branch_work`, `merge_conflict`, `missing_hardening`, `missing_plan` and `event_wait`. `archived_branch_work` marks a change the engine set aside from an archived thread's unproposed branch work. `missing_plan` marks the engine's message to a coding agent whose turn-end work it withheld (`ProposalWithheld`), because the branch had no approved implementation plan. `missing_hardening` marks the boundary before an *Apply* hardens a change. It also marks the engine's message to a coding agent whose turn-end work it withheld because the work never ran `/harden`. `plugin_auto_update` is historical and no longer written.

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

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `MessageReceived` | A user, upstream workspace, parent thread or the engine submitted text into the thread. Stamped at the HTTP boundary in `api/chat.rs::chat_submit`. Its projection **clears the thread's stored compose draft** and advances the *compose epoch*, so a draft write composed before the send cannot apply after it. It broadcasts a `ThreadComposeChanged` with the emptied state and new epoch, so peers drop the draft live. It broadcasts on every send to an existing thread, even with no stored draft: the epoch moved, and a client's write may be in flight. A message that CREATES the thread announces nothing, since no device knows the thread yet. Carries the turn's resolved `model`, `reasoning_effort` and `provider`. `provider` is the backend chosen for a multi-backend model, so the thread keeps it even if the model's *preferred provider* changes. Absent means the model's own row decides. | one-per-turn | yes | yes |
| `QueuedMessageRemoved` | A user removed a queued follow-up before its agent read it: a chat follow-up the loop had not ingested, or a Claude Code follow-up the agent confirmed it dropped. An append-only marker over the original `MessageReceived`. Renderers hide the matching message only while it has no steps, and the loop skips the matching injected prompt. Carries `removed_message_id: Uuid` (the queued `MessageReceived`'s event id), plus `actor` / `channel` from `EventMeta`. | per-action | yes | yes |
| `TextStreamed` | A complete chunk of assistant text was committed to the thread (post-stream finalize for chat; one event per appended chunk). A chat question card's `message` lands as one, just before the card. | high-volume-streaming | yes | **no (blocked)** |
| `ThoughtStreamed` | The model emitted a `thinking` / reasoning block (extended-thinking models). One event per chunk. Legacy alias: `Thinking`. | high-volume-streaming | yes | **no (blocked)** |
| `ContextCaptured` | One snapshot of the context the engine assembled for one LLM call: prompt sections, tools, estimated tokens, and real `usage` when the provider reports it. The modal reads these to show context drift. **`purpose` says which call it was** (§ Which call each purpose names). **A trigger on `ContextCaptured` sees auxiliary calls too**, so add `condition: { purpose: "turn" }` for agent turns only. Payload, `reconstructed` rows and `usage.modality`: § `ContextCaptured`. | per-action | yes | yes (use condition) |
| `MemoryRecalled` | The engine's **automatic pre-turn recall**, on the Classic *memory module* only: a Tree workspace's turn reads its memory views instead and emits none. Before the model saw anything, a classifier derived sub-queries, the chat-side memory consumer vector-searched long-term memory, and the hits went into the turn's context. Carries `results: usize` (how many were injected), `queries: Vec<String>` (the sub-queries) and `memories`. Each memory has `id`, `topic`, `summary`, `src_created_at` and `source`, in the order the model saw them, so the length equals `results` (absent on older rows). The step detail lists `memories` as links to their sources. **Not the agent's own lookup**: the agent's mid-turn `memory` tool `search` arrives as a `ToolCalled`. Subscribe here for "the engine recalled something", to `ToolCalled` for "the agent went looking". Legacy alias: `MemorySearched` (renamed because the two names read as the same event). Old rows still decode and render, but **subscriptions do not follow a rename**: a trigger or `await_event` on `MemorySearched` stops firing. Re-point it at `MemoryRecalled`. | per-action | yes | yes (use condition) |
| `ToolCalled` | The chat agentic loop invoked a tool (`name`, `args`, optional `description`). Coding-agent tool calls are `CodingAgentToolCalled` instead. | per-action | yes | yes (use condition) |
| `ToolResult` | The result returned to the chat loop for a prior `ToolCalled`. Carries `result: String`, `images`, `success: bool` (default true), and `tool_called_event_id: Uuid`. **Pair a result with its call by that id, never by position** (§ `ToolResult`). **Inline image bytes are stubbed out of `result`**, e.g. `[image image/png, 641.2 KB omitted, not embedded in event]` or `[screenshot image/png, 1.5 MB omitted, not embedded in event]`, then the page DOM. The calling model saw the real image. A stub means the image was shown and not persisted, never that it failed. | per-action | yes | yes (use condition) |
| `BackgroundBashStarted` | A long-running task was spawned via `run_bash_background` (shell command), a coding agent's `lucidos background-task run` (the same, in its worktree), OR `run_python_background`. For Python the engine wraps the venv script as `bash -o pipefail -c "<venv-python> <script>"` and uses the same registry. The `command` field captures the exact shell invocation. A `BackgroundBashCompleted` follows. | per-action | yes | yes |
| `BackgroundBashCompleted` | The task ended: natural exit, signal death, watchdog timeout, `bash_kill`, or the engine going away under it. Carries `exit_code: Option<i32>` (set **only** for a normal exit), `signal: Option<i32>` (set only for a signal death; omitted otherwise), `stdout`, `stderr`, `timed_out: bool`, `killed: bool`, `abandoned: bool`. Read the status per § `BackgroundBashStarted` / `BackgroundBashCompleted`: both codes null is never success, and `abandoned` is not `killed`. Every started task on a live thread reaches exactly one of these, so a subscription never waits on an event nobody sends. **A thread's own stop does not wake it** (ADR 0369): `bash_kill`, `lucidos background-task stop` and `lucidos hardened mark` stand down the stopping thread's waits on the task before signalling it, so the killed completion matches nothing there. The task's owner still hears about a stop another thread issued, and the watchdog's timeout, Discard and Archive deliver as before. Emitting it does NOT evict the in-memory registry entry. A completed task stays drainable for a few minutes, so a `bash_output` at the completion instant still gets the final tail. `bash_output` falls back to this row once that window closes. Same shape for `run_bash_background` and `run_python_background`. | per-action | yes | yes |
| `ResponseGenerated` | The chat agentic loop ended with an assistant response: the chat-mode terminator. Carries `text` (`#[serde(skip_serializing_if = "is_empty_str")]`), `images`, `model`, `reasoning_effort`. **`text` may be empty** on a *benign empty completion*, e.g. Gemini `finishReason: STOP` after successful tool calls. The thread then completes Idle, not red (§ `ResponseGenerated`). | one-per-turn | yes | yes |
| `ResponseCanceled` | User clicked Cancel, clicked Apply / Discard / Archive on a still-running session, or posted a follow-up that interrupted a mid-turn Codex turn. Carries `cause: CancelCause` (`UserStop` / `UserAction` / `SupersededByFollowup` / `Unknown`). Always emit via `thread_events::emit_response_canceled`, which is idempotent against pre-emitted terminators (the `/api/v1/restart` race). | one-per-turn | yes | yes |
| `ResponseAborted` | System-driven termination: engine shutdown, safety net (non-watchdog), recovery sweep, OS signal, stale-projection settle. Carries `cause: AbortCause` (`EngineShutdown` / `SafetyNet` / `RecoveryAfterRestart` / `ProcessKilled` / `StaleSettle` / `SessionDropped` / `Unknown`). Always emit via `thread_events::emit_response_aborted`. When a hung-subprocess watchdog interrupts a coding agent (not a crash or driver death), the engine emits `ContinuationRequested{auto_recovery_after_hang}` instead of `ResponseAborted{SafetyNet}`. The thread then auto-resumes. Two watchdogs can fire that path (see `ContinuationRequested`). | one-per-turn | yes | yes |
| `ResponseFailed` | Hard failure mid-turn: upstream API error, panic, OOM-killed bash, empty assistant text on a non-cancel turn (`agent_session::lifecycle::classify_result` does this for coding-agent threads too). Carries `error: String`. An empty chat completion lands here only for a *genuine* failure: output **truncated**, **blocked** by a safety classifier, **dropped** (billed but nothing parsed; Anthropic-only), or an **unrecognised** stop reason. A clean empty stop emits an empty `ResponseGenerated` instead. A reply a safety classifier stops **partway** also lands here, and its streamed `TextStreamed` text stays on screen with no `ResponseGenerated`. `classify_empty_completion` and `normalize_finish_reason` (`agentic_loop/helpers.rs`) treat every provider and thread type alike. | one-per-turn | yes | yes |
| `PromptInjected` | An injected prompt reached the live agentic loop: a human interjection (a message sent mid-turn), an agent's follow-up into its child thread, or an engine note (a resume note, an event-wait re-entry anchor). `mode` names the sender. Only a `Human` one counts as a user action for the drawer's recency. Retired name: `UserPromptInjected` (ADR 0389). Carries `text`, `mode: ActorMode`, optional `origin`, optional `injected_message_id`, optional `delivered_event_id`. The loop frames the text before the model sees it (`framed_injected_prompt`, `agentic_loop/helpers.rs`), keyed on `mode` **and** on whether it lands mid-turn or starts a turn. Mid-turn, a `Human` message is an interjection to answer *and then resume the work in progress*. A redirect still overrides, but answering alone does not end the turn. `Agent`/`Engine` messages are a system update to fold into the response in progress. An injection the previous turn left undrained runs as its own turn (`api::chat::process_orphan_chain`) with no resume directive, since that turn's work is over. **Every** orphan in such a batch is announced, the first included (`announce_orphan_batch`). A restart also ends a turn undrained. Then the queued messages come back from the event store, not the dead channel (`chat::queued_recovery`, ADR 0236). They ride the resume's own turn after the engine note, announced the same way, so the client absorbs each into its own message panel. The re-processed turn reuses the persisted `MessageReceived` as its starter event. So this is the only event in that turn whose lifecycle rule sets the thread back to `running`. The client uses `injected_message_id` to absorb it into that message's panel instead of rendering a second one. The event always stores the user's raw `text`, never the framing. | per-action | yes | yes (use condition) |
| `ImageDescribed` | A background Flash call described one image attached to a `MessageReceived`, one event per `user_image_hashes` entry. Emitted from the agentic loop after iteration 1 of a chat turn. A mid-turn message is injected instead, so that path emits from the chat injection fast-path as a detached task. Carries `source_event_id: Uuid`, `hash: String`, `description: String` (after the `is_bad_image_description` filter) and `model: String`. The `description` is indexed into memory (§ `ImageDescribed`). | per-action | yes | yes (use condition) |
| `ConversationSummarized` | An auxiliary model compressed this thread's older **assistant** turns into one cached paragraph (ADR 0102). On the Classic *memory module* only: a Tree workspace reads its thread memory view instead, and emits none. Emitted from `load_chat_history` during turn setup, only when the summariser succeeds. Carries `summary: String` (exactly as `[CONVERSATION HISTORY]` renders it), `covers_through_event_id: Uuid`, `covered_count: u32`, and `model: String`. Not indexed into memory, since the turns it summarises already are. See § `ConversationSummarized`. | per-action | yes | yes (use condition) |
| `TodoListWritten` | The *Lucidos Agent* called the `todo_write` LLM tool, OR the engine's `todo_consumer` settled the still-open items when the thread stopped working the list. Replace-whole-list: `items: Vec<TodoItem>` is the new complete *todo list*, superseding any prior `TodoListWritten` in the thread. An optional `notes: String` (ADR 0085's *todo notes*) is replaced with the items. It is omitted, not null, when there are none and on older rows. No tool schema offers the field, so it arrives only where the agent writes one anyway. The engine's settle carries it through untouched, so it never erases what the agent wrote. Each `TodoItem` has `content: String` (imperative, "Run tests"), `active_form: String` (present continuous, "Running tests"), `status: TodoStatus` (`pending` / `in_progress` / `completed` / `waiting` / `abandoned`, snake_case on the wire). The LLM tool handler enforces ≤ 50 items and at most one `in_progress`. It rejects BOTH `waiting` and `abandoned` (engine-only). An empty list is valid and means "cleared". `todo_consumer` subscribes to every terminator (`ResponseGenerated` / `ResponseCanceled` / `ResponseAborted` / `ResponseFailed`). It re-emits the latest list with every open item settled, so the panel is honest once a response ends. **The settled status answers "parked or walked away?":** `waiting` when the thread still holds a live *event wait* at that terminator, `abandoned` otherwise. (`await_event` does not hold the turn, per ADR 0049, so a subscribed thread terminates normally and sleeps.) `waiting` is still open, so a wait that resolves without the agent resuming the list settles to `abandoned`. `abandoned` is terminal; a later subscription never reverses it. The consumer's **second trigger is `EventWaitCanceled`**. `EventWaitDelivered` and `EventWaitExpired` each write a `PromptInjected` re-entry anchor, so the re-entered turn's terminator settles the list. A cancel re-opens nothing, and would otherwise leave an idle thread reading `waiting` forever. That settle is skipped while a turn still owns the list (status `running` / `waiting_for_user_answer` / `paused`, i.e. the agent standing its own watch down mid-turn). Cancelling N subscriptions in one cascade writes at most one settle: at cancel k the later waits are still unresolved, so the target stays `waiting` and the already-settled short-circuit holds. Chat-agent tool only: *coding-agent threads* render backend-native todo output. Under *self-curated context mode* there is no `todo_write`; the same event comes from the `[TODO]` heading of the agent's *working understanding*. UI: the frontend finds the most recent `TodoListWritten` and renders it in the prompt-bar collapsible panel. Abandoned rows get a dashed strike-through and an `abandoned` tag; waiting rows get a clock marker, full-strength text and a `waiting` tag. | per-action | yes | yes (use condition) |
| `WorkingUnderstandingWritten` | The *Lucidos Agent* wrote its *working understanding*, as a marked span of ordinary text in its reply. Carries `document: String`, the whole of what the thread now holds, superseding any prior row. A replace sets the document; an add appends to its body and constraints. Only the body and constraints are stored. The checklist goes to `TodoListWritten`, and held-open addresses are applied and dropped, so a rewrite cannot re-assert an old keep. An entry may carry the `evt-<hex>` address of its event, readable through `events(action="query", event_id=…)`. Nothing in the body is parsed, so a missing address costs a read, not a write. Only a line under `[KEEP OPEN]` must be an exact address; one that is not is refused with a fault. Thread-scoped, NOT long-term memory: world facts are the extractor's job. Emitted only where the *self-curated context mode* preference is on, and only from chat and trigger threads. Rendered in the transcript as a folded step, never as chat prose. | per-action | yes | yes |

Terminator set for chat-mode (`TERMINATOR_EVENT_TYPES` constant): `ResponseGenerated`, `ResponseCanceled`, `ResponseAborted`, `ResponseFailed`. Used by `has_terminator_for` for idempotent terminator emission.

**One turn, one terminator, even during a restart.** An interruption produces two would-be terminators. The engine pre-emits `ResponseAborted` for the turn it tears down, and the loop's cancel arm fires moments later. Both name the same turn through `request_event_id`, so `has_terminator_for` sees the abort and `emit_response_canceled` skips. That holds only because the pre-emit takes the anchor from the running turn (`engine::in_flight_request_event_id`), not the newest `MessageReceived`. Guessing stacked "Paused by restart" on "Response canceled" after a mid-turn follow-up or a `ContinuationStarted` turn.

The rule covers all three out-of-loop emitters: the `/api/v1/restart` teardown, the 60 s stuck-turn eviction, and the shutdown sweep.

**A coding-agent session needs another way, because its request id cannot do that job.** A live session keeps ONE `request_event_id` across every follow-up turn. So "a terminator exists for this request id" would read turn one's `ResponseGenerated` as covering turn two, and swallow a real terminator. Its suppression is scoped to the *turn* instead: a `ResponseAborted` that out-sequences the thread's newest start event already covers the turn, so the session emits nothing. Start events are `MessageReceived`, `CodingAgentUserMessageSent`, `TriggerStarted`, `ContinuationStarted` and `OrphanRecoveryStarted`.

Two checks apply. The in-memory `AgentSession::external_terminal_emitted` flag is the fast path when the session was already registered. The events-table check covers a boundary emitted **before the session existed at all**. Both restart emitters iterate a snapshot of `agent_sessions`, so they miss a session still spawning. Without that check, a *Switch to new version* during a spawn stacked a `ResponseCanceled{user_stop}` beside "Paused by restart". The resume gate reads a cancel as "turn ended", so the next boot declined the promised auto-resume.

## Resume / continuation

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `ContinuationStarted` | Resume-after-abort boundary. Opens a new exchange whose body is the rerun (chat: a fresh LLM call; coding agent: resume into the same `cc_session_id` when the backend has one). **Channel-agnostic**: emitted on chat, trigger and coding-agent paths alike, so it says nothing about a thread's type (see below). Carries optional `branch`, engine-stamped `origin`, and `reason`, which mirrors `ContinuationRequested.reason` so a hang or stray-signal recovery is not labelled an engine restart. Aliases for old DB rows: `SessionRecovered`, `SessionResumed`. | lifecycle | yes | yes |
| `SessionStarted` | A coding-agent process spawned. Carries `session_id` (backend session id), optional `branch`, optional `repo_id`, plus *coding-agent-thread* discriminators. `coding_agent_kind`: `"lucidos" \| "app" \| "external"`, default `"lucidos"`. `coding_agent_folder`: the folder the spawn targets, `<ws>/data/apps/<id>/` for App, else the repo root. `app_id`: set only for App. `coding_agent`: `"claude-code" \| "codex"`, default `"claude-code"`, the backend driving the thread, locked in by the first SessionStarted via the `thread_summaries.coding_agent` projection. Legacy rows without these decode as Lucidos / Claude Code via serde defaults. A session start consumes the thread's prompt. So its projection **clears the thread's stored compose draft**, advances the *compose epoch* and **broadcasts a `ThreadComposeChanged`** with the emptied state and new epoch. Both are gated on a coding-agent event: a chat or trigger `ContinuationStarted` shares this arm and must leave the draft and epoch alone. | lifecycle | yes | yes |
| `SessionEnded` | A coding-agent thread is truly done (terminal-only). Carries `reason: SessionEndReason` (`Shutdown` / `Panic` / `Closed` / `StaleResume` / `LegacyNonTerminal`). `StaleResume` is the one transient case and does NOT settle the thread. The projection stays `running` while the caller re-spawns once with a fresh session (chat and the continuation/spawn consumer both do), and the frontend skips the AbortPanel. A caller that does not retry leaves the thread `running` with no subprocess, so the retry is part of the contract. | lifecycle | yes | yes |

## Coding agent (Claude Code / Codex)

The `CodingAgent*` family covers Claude Code and Codex. Variants carry `coding_agent: CodingAgent`, default `ClaudeCode`, with `#[serde(alias = "agent")]` so pre-rename rows decode. Each variant has a `#[serde(alias = "ClaudeCode<X>")]` for its pre-rename name. Write new code against the `CodingAgent*` form.

**`system-knowhow/coding-agent-events.md` has the full payload shapes, the `CodingAgentIdled` field reference, the `UserQuestion` vs `CodingAgentPermission` distinction, and the no-`CodingAgentErrored` gap.** This table is the index.

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `CodingAgentUserMessageSent` | A user message was relayed into the agent's input stream. | one-per-turn | yes | yes |
| `CodingAgentPromptSent` | An engine-synthesized prompt was injected (orphan recovery, hardening retrigger, merge-conflict explainer, post-question continuation). Carries `origin: Option<MessageOrigin>`; an automated prompt (hardening retrigger, merge conflict) always carries an engine origin with its reason. Audit-only, not rendered in chat. | per-action | yes | yes (use condition) |
| `CodingAgentInputRead` | The coding agent read an input the engine forwarded. Carries `input_event_id` (the `MessageReceived` it read, or the `ChildThreadCompleted` of a child wake). Carries `started_turn: true` (omitted when false) when the read opened a new turn, such as a queued message after a Stop; that read sets the thread running again. See [coding-agent-events](coding-agent-events.md). | per-action | yes | yes |
| `CodingAgentTextStreamed` | One chunk of the coding agent's assistant text. Join a turn's chunks with no separator: each new text block already starts a new paragraph. A chunk never holds whitespace alone: a paragraph break leads the next chunk instead. Older rows can still hold a bare `"\n\n"`, or two blocks with no break between them. A chunk a Claude Code sub-agent wrote carries `parent_tool_use_id` and is the sub-agent's narration, not the reply. A chunk flagged `progress_note: true` is a progress note: the provider's short summary of longer text the agent wrote before a tool call. The user saw only the summary. | high-volume-streaming | yes | **no (blocked)** |
| `CodingAgentThoughtStreamed` | One chunk of the coding agent's streamed reasoning (CC's `thinking_delta`; Codex's `item/reasoning/*Delta` or `reasoning` item). Coalesced before persistence. Rendered as the live "Thinking" step's content. | high-volume-streaming | yes | **no (blocked)** |
| `CodingAgentToolCalled` | One coding-agent tool invocation. Carries `name`, `args`, optional `description`, `tool_use_id`. A call a Claude Code sub-agent made also carries `parent_tool_use_id`, the id of the `Agent` call that spawned it. A Claude Code call carries `api_call_id`, shared by every tool call of one API call and by its `ContextCaptured`. | per-action | yes | yes (use condition) |
| `CodingAgentToolResult` | The result returned to the coding agent for a prior `CodingAgentToolCalled`. Same `tool_use_id`, `name` and `parent_tool_use_id`. `result` holds the whole output (rows before full storage kept 200 chars). | per-action | yes | yes (use condition) |
| `CodingAgentIdled` | **The coding-agent turn-boundary marker.** Emitted at the end of every coding-agent turn whose Result was not an engine-shutdown abort. Carries `has_changes`, `is_external_repo`, `requires_restart`, `cc_session_id`, `coding_agent`, optional `reason`, optional `worktree_path`, optional `worktree_head_sha`, and `bg_bash_pending`. That last is a history flag: true when the turn idled with a chat-agent `run_bash_background` task running. It **no longer gates proposal or drives any UI**; harden-at-apply covers correctness. | one-per-turn | yes | yes |
| `CodingAgentSettingsChanged` | User changed model or reasoning effort mid-session. Also emitted once at backend init with `cc_session_id`, `claude_config_dir` (where the session's transcripts live) and `claude_config_dir_explicit` (whether `CLAUDE_CONFIG_DIR` was set). That makes them durable before the first `CodingAgentIdled`. The session id lets a mid-turn engine restart still resume. The config-dir fields let the resume replay the same Claude Code profile, even if the user toggled the env var meanwhile. | lifecycle (rare) | yes | yes |
| `CodingAgentPermissionRequest` | A coding agent asked to confirm a tool call, on one of two raise paths. Claude Code's MCP permission-prompt subprocess fires for a path outside the session's working directories, or `.git/` inside the worktree. Those directories are the worktree, the workspace's `data/` tree and `/tmp`; an in-worktree write, including under `.claude/`, is auto-allowed before any card. Under the `auto` permission mode CC's classifier decides instead. The Codex app-server approval bridge fires for a sandbox-escaping `command_execution` or an out-of-worktree `file_change` under `approvalPolicy: on-request`. The exec escape-hatch protocol emits none. | per-action | yes | yes |
| `CodingAgentPermissionResolved` | The above request was answered, or the engine auto-resolved it. Auto cases: recovery (orphaned after restart); supersession (`allowed: false`, `reason: "Superseded by a new message"` when the user types instead of clicking); a **session-ended clear**. The clear fires when the turn idled with the card still open, e.g. a parallel subagent's card outliving the main turn. Its `reason` starts `"Coding agent session ended before answering"`. Carries `allowed`, optional `reason`, optional `persist_scope` (`narrow` / `broad` / `session`). Flips the thread back to `running` **only from `waiting_for_user_answer`**. A resolution on an idle or terminal thread (a stale click, or the session-ended clear) leaves the status unchanged, so a finished thread never becomes a dead `running`. | per-action | yes | yes |
| `MissingHardeningDetected` | A coding-agent session ended without running `/harden`, so the engine auto-spawned a recovery hardening session. Not a session terminator: the thread stays active until hardening finishes. | lifecycle (rare) | yes | yes |
| `ContinuationRequested` | An interrupted coding-agent turn needs to resume without a new user message. The spawn dispatcher picks it up; the event id is the spawn idempotency key. Delivery is guaranteed two ways. The dispatcher subscribes to the bus before its startup backfill, so a request emitted during startup is buffered. And every engine start re-dispatches any unactuated request: no later lifecycle or terminal event, on a thread still `running`. So a request never strands a thread as a running zombie. `reason: String` is one of: `"user_clicked_continue"` (Continue after an engine restart); `"answered_after_idle"` (an `AskUserQuestion` answered after the subprocess was torn down at idle); `"auto_recovery_after_hang"` (a hung-subprocess watchdog found the agent silent past its inactivity limit); `"auto_resume_after_switch"` (recovery resumes an in-flight thread after a user's *Switch to new version*); `"harden_requested"` (the user pressed **Harden** on the *Not ready* strip, and the thread resumes to run `/harden`); `"auto_resume_after_api_error"`. That last one fires when the agent ended a turn on a transient upstream failure it reported itself (`API Error: …`, e.g. a connection closed mid-response). The engine then resumes instead of leaving the thread dead behind the `ResponseFailed`. It is the only BOUNDED reason: at most 3 in a row since the thread's last `MessageReceived` or `ResponseGenerated`, so a broken upstream shows a red dot instead of looping. It is skipped during engine shutdown (post-restart recovery owns those threads) and on a conflict-resolution session (an API drop mid-merge aborts the merge and leaves the change pending). Two watchdogs produce `auto_recovery_after_hang`. The in-loop one sits in `run_session`'s `select!` (10 min, the fast first line). An external scanner task (12 min, ticks every 30 s) catches a `select!` wedged in an event-handler await. They share a gate, and the 2-min grace lets the in-loop one fire first. The gate skips while a tool is in flight (a long `Bash`, an unanswered `AskUserQuestion`), up to a 45-min hung-tool ceiling. Past it, a tool that never returns (a hung sub-agent) fires anyway, after re-confirming the thread is still `running`, so a pending user answer is never killed. A thread may have an in-flight *conflict resolution*: a pending change whose latest merge-lifecycle event is an unpaired `MergeConflictDetected`. Then a **recovery-shaped** continuation (`user_clicked_continue` / `auto_recovery_after_hang` / `auto_resume_after_switch`) re-attaches the merge duty. Never `answered_after_idle`, a different interaction that must not silently land a change. The resumed session runs in the merge worktree with the change bound, so its completion finishes the apply (`ChangeApplied`) or aborts for real. A stray-killed merge session's cleanup skips the failure events, so the pairing stays open for the hand-off. A duty the continuation cannot carry (merge worktree gone, resume failed first) closes loudly with the deferred `MergeResolutionCleared` + `ChangeApplyFailed` pair. Past name `ContinueSignal` is a serde alias for old DB rows. | lifecycle | yes | yes |
| `SideQuestionAsked` | The user asked a *side question* in a Claude Code or Lucidos Agent thread. Carries `side_question_id: Uuid` (named by the client), `question`, any attached `image_hashes`, and `actor` from `EventMeta`. Records a card beside the thread, never a turn: it moves no status, recency, count or section. **No agent sees it**: `query_events`, triggers, event waits and every context builder leave it out (ADR 0320). | per-action | yes | **no (hidden)** |
| `SideQuestionAnswered` | The thread's agent answered the side question with this `side_question_id`. Carries `answer`. Same hiding as `SideQuestionAsked`. | per-action | yes | **no (hidden)** |
| `SideQuestionFailed` | The side question with this `side_question_id` got no answer. Carries `error`. Startup writes one, `"Interrupted by a restart. Ask again."`, for every ask a restart left unsettled. Same hiding. | per-action | yes | **no (hidden)** |
| `SideQuestionDismissed` | The user dismissed the card for this `side_question_id`. The ask stays recorded, so the card collapses to a row that reopens. Same hiding. | per-action | yes | **no (hidden)** |

## Question / permission machinery

Not prefixed `CodingAgent*`, because any agent can use this machinery to ask the user a structured question. Every blocking request below is triggerable, so a trigger can push the user when an agent needs an answer. `triggers.md` has the deep-link pattern (`tap: { kind: 'navigate', to: { target: 'thread', id, event_id } }`).

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `UserQuestionAsked` | An interactive question was raised by Claude Code's `AskUserQuestion` tool, Codex's `ask_user_question` MCP tool (one question per call), or the chat agent's `ask_user_question` tool. `meta.channel` records the lane: `claude_code` (the coding-agent channel, both backends) or `chat`. Resume goes through `POST /api/v1/threads/{thread_id}/answer-question`. The engine branches on the channel: coding agent gets a resume marker plus a `ContinuationRequested` respawn if needed; chat wakes the in-process tool. Carries `tool_use_id`, `cc_session_id` (empty for chat-channel and Codex rows), `question`, `options: Vec<QuestionOption>`, optional `worktree_path`, `multi_select: bool`, and on an *owner approval card* `owner_approval: { verb, target_thread_id? }`. | one-per-turn (of the `Asked` kind) | yes | yes |
| `UserQuestionAnswered` | The user (or the engine, on the orphan-recovery path) supplied an answer. Pairs 1:1 with its `UserQuestionAsked` via `tool_use_id`. Carries `answer: AnswerKind` (`Selected` / `FreeText` / `MultiSelected` / `Canceled` / `Superseded`) and copies `meta.channel` from the `Asked`. A typed `FreeText` or `MultiSelected` may carry the user's `image_hashes`. The last two kinds resolve the question without an answer. `Canceled` is a dismissal or teardown stamp. `Superseded` is a follow-up that could not be the answer and replaced the question (coding-agent lane only). | one-per-turn | yes | yes |
| `OwnerApprovalRequested` | A coding agent asked for an *owner approval* with `lucidos ask-owner-approval` (ADR 0387). Carries `request_id`, `approval: { verb, target_thread_id? }` and `question`, the card text Lucidos wrote. Grants nothing by itself: the card appears when the agent asks `AskUserQuestion` with `request_id` as its question, and that card's `UserQuestionAsked` carries the same `owner_approval`. | per-action | yes | yes |
| `OwnerApprovalSpent` | A thread used the owner's **Allow once** on the act it names, just before the act ran. Carries `tool_use_id` (the approval card), `verb` and optional `target_thread_id`. At most one per card. | per-action | yes | yes |
| `CommandPermissionRequested` | The **command guard** (ADR 0002) paused a chat bash/python tool call in the `IrreversibleDanger` lane to ask the user. That lane means a likely real-world side-effect (mutating HTTP, sending mail, a cloud-CLI mutation) or destruction outside the workspace. A static fast-path settles the obvious safe or catastrophic cases; the LLM **judge** (Phase 3) decides the ambiguous middle. The chat mirror of `CodingAgentPermissionRequest`: same `PermissionCard`, but the agent loop blocks in-process (no MCP subprocess). Carries `request_id`, `tool_use_id`, `tool_name` (a bash/python tool), `command` (the inspected text), `summary` (the card's one-line risk, written by the judge). Chat-channel only; flips the thread to `waiting_for_user_answer`. | per-action (only when the guard is on AND a command hits the danger lane) | yes | yes |
| `CommandPermissionResolved` | The above was answered (Allow once / Deny / Allow for this thread / Always allow) or auto-resolved. Auto reasons: `"Superseded by a new message"` when the user types instead of clicking, or an orphan/cancel reason on restart or Stop. Carries `request_id`, `allowed`, optional `reason`, optional `persist_scope` (`narrow` / `broad` / `session`). Flips the thread back to `running` **only from `waiting_for_user_answer`**; a stale resolution on an idle or terminal thread leaves the status unchanged. | per-action | yes | yes |
| `McpPermissionRequested` | The Lucidos Agent (chat) paused an **MCP server tool** call to ask the user: the MCP mirror of `CommandPermissionRequested`. Same `PermissionCard`; the agent loop blocks in-process. Carries `request_id`, `tool_use_id`, `server_id` (MCP registry key), `server_name` (human label), `tool_name` (bare MCP tool), `arguments_summary`. Chat-channel only; flips the thread to `waiting_for_user_answer`. **Skipped (no event, auto-approved) in two cases**: a non-interactive **trigger** thread, and a server with the `auto_approve` flag set. | per-action (only when the call isn't pre-authorized) | yes | yes |
| `McpPermissionResolved` | The above was answered (Allow once / Deny / Allow for this thread / Always allow this tool / Always allow this server) or auto-resolved (superseded / orphan / cancel). Carries `request_id`, `allowed`, optional `reason`, optional `persist_scope`. Scopes: `narrow` → `Mcp(server:tool)` and `broad` → `Mcp(server:*)`, both persisted to the workspace's `mcp-allowed-tools`; `session` → in-memory per-thread. Flips the thread back to `running` **only from `waiting_for_user_answer`**; a stale resolution on an idle or terminal thread leaves the status unchanged. | per-action | yes | yes |
| `CommandCheckpointed` | The **command guard** (ADR 0002, Phase 4) bracketed a `ReversibleDanger` command (in-workspace deletion or overwrite) with two snapshots of git-visible content. A **pre** image goes on a safety ref before it runs, and a **post** image after. The diff tells the engine which files the command created, overwrote and deleted, so the card offers Undo and a view of the change. Emitted **after** the command returns, and only when the images differ. A command that changed nothing git-visible (often a gitignored target) emits nothing, since Undo would do nothing. A failed snapshot also emits nothing and lets the command run unguarded. Carries `checkpoint_id` (the ref key), `command` (the inspected text), `summary` (the card line), and the counts `restores` / `removes`. Those are what Undo would put back and what it would delete as command-created; both 0 on older events. Does not change thread status. | per-action (only when the guard is on AND a command hits the reversible lane AND it changed something git-visible) | yes | yes |
| `CommandCheckpointReverted` | The user clicked Undo on a `CommandCheckpointed` card (or the engine resolved it). Files the command created were removed, and files it deleted or overwrote came back from the pre image. A path is touched only if it still holds what the command left, so a later edit survives. A checkpoint with no post image restores the whole pre image instead. Both refs are kept, so the card's diff stays viewable. Carries `checkpoint_id`; stamped with the original turn's `request_event_id` so it groups with its checkpoint (the card renders reverted). | per-action | yes | yes |
| `WidgetShown` | A *widget* was shown at this point in the thread: `create_app` with `kind="widget"` made it here, or `widgets(action="show")` (UI, CLI or agent) put it here. Carries `app_id`, plus the instance's optional `params` object and `label` (ADR 0415). A row with no `params` is the instance with none. The widget's files stay in its app folder. Draws the widget inline at this turn, as its *widget card*. It adds no chip to the thread's *widget shelf*; only a pin does (ADR 0407). | per-action | yes | yes |
| `WidgetPinned` | "Pin to shelf": the user, or `widgets(action="pin")` from the agent or CLI, put a *widget instance*'s chip on this thread's *widget shelf*. In Home, which draws no shelf, the menu reads "Pin to Home" and the pin lists the instance in Home's long-press menu. Carries `app_id` and the optional `params` and `label`. A pin alone adds the instance, as from a *widget embed*. No file or commit changes. Legacy alias: `WidgetRestored`. Old rows still decode, but a trigger or `await_event` on the old name stops firing. | per-action | yes | yes |
| `WidgetUnpinned` | "Unpin from shelf": the user, or `widgets(action="unpin")`, took a *widget instance*'s chip off this thread's *widget shelf*. Carries `app_id` and the optional `params` and `label`. The widget card stays, and no file or commit changes. Legacy alias: `WidgetHidden`. | per-action | yes | yes |
| `McpConsentRequested` | Legacy audit-log entry (`tool`, `args`) from the pre-card MCP consent flow. No longer emitted: chat MCP consent uses `McpPermissionRequested` / `McpPermissionResolved`. Kept for replay of historical rows. | lifecycle | yes | yes |

## Form requests

A *form request* is something the agent put in front of the user to fill in or confirm. Each carries a `request_id` and a `payload` string (the JSON the client renders from, never a secret). It stays open until exactly one `FormRequestResolved` names its `request_id`. An open request changes no thread status.

Only the tool named in a row opens its form. A `run_bash`, MCP or fetched result that starts with a form-request prefix opens nothing. The request is persisted, so a client that missed its stream frame still finds it through `GET /api/v1/form-requests/pending`, read on every stream open. The transcript shows each request in its turn, with an Open button while it waits.

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `CredentialRequested` | `request_credential`, `connect_oauth_account` or `configure_email` needs a credential the user must type or confirm. `payload` holds `service`, `prompt`, `auth_type`, and optionally `base_urls`, `defaults`, `env_var_name`, or for a widening `existing_credential_id` and `adding_base_urls`. Resolved by the credential save or by `POST /api/v1/form-requests/{request_id}/cancel`. Legacy aliases: `CredentialPromptRequested`, `CredentialRequest`. | per-action | yes | yes |
| `PluginInstallRequested` | `install_plugin` or `update_plugin` staged an install. `payload` is the preview (manifest, file list, overwrites, optional setup). `request_id` equals its `install_id`. Resolved by `POST /api/v1/plugins/install/{install_id}/{confirm\|cancel}`. Legacy alias: `PluginInstallRequest`. | per-action | yes | yes |
| `PluginUninstallRequested` | `uninstall_plugin` staged an uninstall. `payload` is the preview (plugin name and version, files present and missing). `request_id` equals its `uninstall_id`. Resolved by `POST /api/v1/plugins/uninstall/{uninstall_id}/{confirm\|cancel}`. Legacy alias: `PluginUninstallRequest`. | per-action | yes | yes |
| `EmailConfirmRequested` | `send_email` wants the user to confirm a draft. `payload` is the draft. Resolved by `POST /api/v1/email/send` or the cancel route above. Legacy alias: `EmailConfirmRequest`. | per-action | yes | yes |
| `OAuthAuthorizationRequested` | `connect_oauth_account` asks the user's device to open the provider's authorization page. `payload` is `{target: "url", url, purpose: "oauth"}`, and the meta `actor` names the device that opens it. The flow's listener resolves it when its 120 s wait ends. | per-action | yes | yes |
| `FormRequestResolved` | A form request closed. Carries `request_id` and `outcome`: `completed` (saved, confirmed, sent, authorized), `canceled` (the user declined, or the provider refused), `superseded` (a newer request for the same credential or plugin in the thread, or a new user message), `expired` (plugin staging or the OAuth listener is gone: TTL, timeout, restart). Emitted once per request. | per-action | yes | yes |

The `QUESTION_OVERTAKEN_EVENT_TYPES` constant lists the events that mean a `UserQuestionAsked` is no longer the thread's latest interactive point. Once one lands after a question, the next typed text starts a fresh follow-up, not a `FreeText` answer. Two categories:

- **Terminal**: `ResponseAborted`, `ResponseCanceled`, `ResponseFailed`, `CodingAgentIdled`.
- **Agent progression**: coding agent (`CodingAgentTextStreamed`, `CodingAgentToolCalled`, `CodingAgentToolResult`, `CodingAgentPromptSent`) and chat (`TextStreamed`, `ThoughtStreamed`, `ToolCalled`, `ToolResult`).

The coding-agent progression set defends against a race. A coding agent can emit a question alongside sibling tool calls, and the siblings keep emitting while the question blocks. Without this filter, the user's next comment would be silently absorbed as a `FreeText` answer to the dead question.

An answer carrying composer text (`FreeText`, or `MultiSelected` with `text`) **clears the thread's stored compose draft**. It does so only when the draft is exactly what was submitted (trimmed text compare, no attached images). It also **broadcasts a `ThreadComposeChanged`** with the empty state and the new *compose epoch*. So every device learns live, and a draft write composed before the answer cannot apply after it. This path emits no `MessageReceived`, so without it the draft would re-sync to every device. A click-only answer submits no text, so it clears and broadcasts nothing, and another device's draft survives.

The FreeText fast-path also requires a **human-authored** follow-up (`ActorMode::Human`). Agent- and engine-driven re-entries are not the user's answer and fall through. Example: a **child-thread completion** re-opens the parent via `notify_parent_of_child_completion` with `ActorMode::Agent`, feeding a `[CHILD THREAD COMPLETED] …` block. Without the guard, that block became a bogus `UserQuestionAnswered { FreeText }` (actor = `thread_link`/`child`) and killed the user's open question. Now it falls through to the injection fast-path (queued as `ReentryFromEngine`). The question stays live, and the child's result runs right after the user answers.

## Thread lifecycle

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `ThreadStarted` | A thread was created in `composing` state (debounced first user input on a fresh compose). Carries `mode: String` (initial compose mode), optional `actor`. | lifecycle | yes | yes |
| `ThreadDiscarded` | A composing thread was explicitly discarded (DELETE /threads/:id). Terminal: the state-machine guard rejects all later compose mutations with 410 Gone. | lifecycle | yes | yes |
| `HomeThreadCreated` | The workspace's home thread was created: an active chat thread titled "Home", with no message yet. The engine emits it once per workspace: at its first boot, or at the first model call no thread made, whichever comes first. No payload. | lifecycle | yes | yes |
| `ThreadTitleGenerated` | The title-generation pass produced a title for the thread (background, after enough body to summarize). Never on the *home thread*, which only the user names. | lifecycle | yes | yes |
| `ThreadTitleRenamed` | The user manually renamed the thread. | lifecycle | yes | yes |
| `ThreadSaved` | User pinned the thread. A pinned thread is never archived, so pinning an archived thread also moves it back to the inbox. Empty payload. | lifecycle | yes | yes |
| `ThreadUnsaved` | User unpinned the thread. Empty payload. | lifecycle | yes | yes |
| `ThreadArchived` | The thread was archived. Empty payload. `actor` says who: a device for the Archive button, or `{"kind":"api","mode":"agent","source_thread_id":…}` for an *agent archive* by that thread (ADR 0310). The engine never archives a thread by itself. The user's Archive unpins a pinned thread. An agent archive refuses one (`thread_pinned`), and an unattended trigger run never archives one. | lifecycle | yes | yes |
| `ThreadArchiveRequested` | The thread's own agent asked to be archived once its turn ends (`threads` 'archive' with `current`). Empty payload; `actor` names the agent thread. The archive lands as a `ThreadArchived` after the thread settles, and a newer `MessageReceived` closes the request. | lifecycle | yes | yes |
| `ThreadUnarchived` | The user moved an archived thread back to the inbox: Archive all's **Undo**, or **Move to Current**, which takes the sub-threads too (`POST /api/v1/threads/unarchive`, ADR 0349, ADR 0378). Empty payload; `actor` is the user's device. Bumps no recency, so the thread returns to where it sat. | lifecycle | yes | yes |
| `ThreadReadRequested` | The yes half of a turn's *read decision*: the thread's own agent asked the user to read its latest reply (`request_read` with `read: true`, or `lucidos request-read yes` from a coding agent, ADR 0409). Empty payload; `actor` names the agent thread, and is absent when the engine forced the decision. Sets the *read request*, which lists the thread in the drawer's Review group once its turn ends. A change that already lists the thread replaces it: a ready change, or work held for a missing harden (ADR 0421). A trigger run that asks lands in the inbox rather than Archive. Never counts as attention. | lifecycle | yes | yes |
| `ThreadReadNotRequested` | The no half of a turn's *read decision*: the agent decided its reply needs no reading (`request_read` with `read: false`, or `lucidos request-read no`). Every turn that ends normally records one decision. Empty payload; `actor` names the agent thread, and is absent when the engine forced the decision. Projects nothing: it never clears an unseen request, and a trigger run that decides no stays archived. | lifecycle | yes | yes |
| `ThreadReplySeen` | The user saw the reply a *read request* pointed at: its end stayed on screen for the seen dwell (`POST /api/v1/threads/:thread_id/read-request/seen`). Carries `seen_version`, the thread's summary version the client saw; `actor` is the user's device. Recorded only while that version still holds a pending request, and clears it. A human message or an archive clears a request too, with no event of its own. | lifecycle | yes | yes |
| `ThreadTriageProposed` | The Lucidos Agent proposed a *thread triage* in this thread (`threads` 'triage', ADR 0349). Carries `entries: [{thread_id, action, reason}]`, where `action` is a triage action name. `apply_triage` acts only on these threads, and only after a user reply newer than this event. `actor` names the agent thread. | lifecycle | yes | yes |
| `ImageUploaded` | A user attached an image to a compose draft (POST /api/v1/threads/:id/blobs). Carries `hash` (sha256, sole identity), `mime`, `byte_size`, optional `actor`. Bytes live exactly once at `data/blobs/<hh>/<hash>.<ext>`. | per-action | yes | yes |
| `TriggerStarted` | A scheduled or event-driven trigger run started. Carries `trigger_id`, optional `trigger_name`, optional `prompt`, optional `invocation: TriggerInvocation` (`Schedule` or `Event { event_type, event_id?, thread_id? }`). That `thread_id` is set only for thread-scoped source events, and script triggers see it as `TRIGGER_EVENT_THREAD_ID`. Also optional `origin`, `go_to_review: bool`, optional `model`, `reasoning_effort` and `provider`. Wire aliases: `task_id`, `task_name` (from when triggers were "scheduled tasks"). `model` / `reasoning_effort` / `provider` record what the fire ran on: the trigger's own pin, else the account chat default; for the provider, the model's preferred route. This is a trigger thread's *starter* event, with no `MessageReceived`, so per-thread model memory reads these fields. A follow-up on a trigger thread reuses the fire's model instead of the account default. Absent on older runs. | lifecycle | yes | yes |
| `TriggerCompleted` | A trigger run finished. Carries `trigger_id`, optional `trigger_name`, optional `result_summary`. Same aliases. `result_summary` is always a non-empty, single trimmed line. A script that exits 0 with no stdout falls back to its last non-empty line, else `"<name> completed (exit <code>, no output)"`. An intent run with no final text falls back to `"<name> completed (no output)"`. So idle-detector triggers never read as a flood of no-op fires to the learning and audit sweeps. | lifecycle | yes | yes |

### Deleting a thread is a `SystemEvent`, not one of these

A thread's own events cannot record its deletion, because the delete removes
them. So `ThreadsDeleted` is a **`SystemEvent`** on aggregate `ops` with
`aggregate_id` `global`. It is persisted and subscribable like any other, and
`condition: { thread_id: ... }` does NOT reach it: the row belongs to no thread.
Subscribe on the type and read `thread_ids` from the payload instead.

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `ThreadsDeleted` | The owner deleted a thread and every sub-thread under it, from the Lucidos UI. Carries `thread_ids` (the whole family, target first), `event_count`, `memory_count`, `worktrees_removed`, `widgets_removed` (the family's own non-reusable widgets, removed from `data/apps/` in one commit), optional `actor`. **No title, no message text, no summary**: it is the only record left, and it must not re-file what the delete removed. | lifecycle (rare) | yes | yes |

Deleting is not archiving. *Archive* moves a thread to the Archive section and
changes nothing about retrievability. Delete removes the rows, and what Lucidos
learned from them, with no undo. It is offered to the workspace owner in the UI
and to nobody else, so no tool, CLI verb or SDK method can reach it.

### Correcting a memory is a `SystemEvent` too

A memory correction belongs to no thread, so `MemoryCorrected` is a
**`SystemEvent`** on aggregate `memory` with `aggregate_id` `global`.

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `MemoryCorrected` | The agent corrected a long-term memory at the user's word, through the `memory` tool's `correct` or `correct_by_id` action. Carries `search_query`, `wrong_fact`, `removed` (each deleted entry's `summary`, `entities` and `source`), an optional `correction` (the fact that replaces it), and `recorded_at`. Every memory rebuild replays it, which is what keeps a corrected fact from coming back. | per-action (rare) | yes | yes |

The Tree *memory module*'s one-time backfill reports on aggregate `memory`
too. Settings → System → Memory draws its progress bar from these.

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `TreeBackfillProgressed` | The backfill moved: it started, a tree began or wrote a summary, one more thread (or the workspace) caught up, a tree failed, or it started or stopped waiting for a background model. Carries `progress`: `done`, `total`, `done_milli`, `nodes_done`, `nodes_total`, `waiting_for_model`, `retrying`, `ready` and `seq`. `seq` is the frame's order, rising while the engine runs. `done_milli` is `done` in thousandths plus each tree under way's built share. `ready` is true once turns use the Tree module while older threads still fill in. | per thread during a backfill | no | no |
| `TreeBackfillCompleted` | The workspace tree and the threads active in the last 7 days are built, so turns now use the Tree module. Older threads keep filling in after it. Carries `total`, the trees the backfill covers. | once per backfill | yes | yes |
| `TreeBackfillReset` | Switching back to Classic cleared a finished backfill, so choosing Tree again catches up first. | rare | yes | yes |

A provider refusing a background task's model reports on aggregate `ops`.

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `ModelNotServedObserved` | A provider answered that it does not serve the model a background task ran on: retired, never enabled, or misspelled. Carries `model`, `provider`, `purpose`, `message` (the provider's failure, with advice where Lucidos has one) and `moved_to`, the model the call retried on. No `moved_to` means the call failed. For six hours an unset default passes over the model; a stored pick keeps failing and gets one notification. | at most once per provider, model and task per six hours | yes | yes |

### Freeing disk space reports as `SystemEvent`s

The *recommended cleanup* behind Disk Usage's "Free up space" button walks
every worktree, so its report belongs to no thread. Its four events are
**`SystemEvent`s** on aggregate `ops` with `aggregate_id` `global`. Each tree
it touches still gets its own `WorktreeCleaned` thread event.

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `RecommendedCleanupStarted` | The user started a pass. Carries optional `actor`. | per-action (rare) | yes | yes |
| `RecommendedCleanupProgress` | After each worktree the pass deals with, skipped ones included. Carries `done` and `total`. A transient frame like `BackupProgress`: subscribe to the terminal events instead. | per-worktree | no | no |
| `RecommendedCleanupCompleted` | The pass finished. Carries `removed_count` (finished worktrees removed), `cleaned_count` (worktrees whose build artifacts were cleared) and `freed_bytes`. | per-action (rare) | yes | yes |
| `RecommendedCleanupFailed` | The pass stopped before it finished. Carries `error`. | per-action (rare) | yes | yes |

## Changes (per-thread coding-agent change proposals)

The change family is per-thread, keyed by `change_id`. `ChangeProposed` is emitted **at most once per coding-agent turn**, at end-of-turn, gated by `idle_change_write`. A clean `Generated` turn proposes its work. A redirect proposes nothing, because the next turn starts at once. Apply and proposal state wait for idle.

**Each coding-agent thread has one *change state***, the `coding_agent_change_state` field on its summary (`codingAgentChangeState` on the aggregate). It is one tagged object:

| `kind` | Means | Detail |
|---|---|---|
| `none` | No work on the branch. | none |
| `unproposed` | Work on the branch that no pending change carries. | `reason`, or `null` |
| `proposed` | A pending change exists, and Apply takes it. | `requires_restart` |

The `reason` is the verdict of the last turn end that withheld the work: `plan_missing`, `plan_awaiting_approval`, `outside_bound`, `hardening_missing` or `turn_incomplete`. A new turn clears it. A `null` reason means no turn end withheld it. The thread is still running, holds a live event wait, works in an external repo, or its work was set aside. Other summary fields already say which (ADR 0400).

**An unfinished turn never proposes.** A user Stop, a failure, an abort or a cut-off at shutdown leaves its work on the branch as `unproposed` with reason `turn_incomplete`. The engine announces it with `ProposalWithheld`. Continue resumes the session, and the next finished turn proposes. This supersedes ADR 0328's "a Stop proposes".

If such a turn moved the branch past a pending or set-aside change, that change is withdrawn (`ChangeWithdrawn`), so Apply never merges partial commits. A change that still carries all the work stays, and nothing is announced.

A clean turn that ends while the thread holds a live event wait only re-syncs an open change. Its proposal waits for the turn the wait re-opens, or for a cancel of the last wait (ADR 0395). Until then the thread cannot be archived or deleted.

The plan floor also withholds. A Lucidos-source branch with no approved implementation plan proposes nothing. Its turn end emits `ProposalWithheld` with the plan reason, and a live coding agent gets a `missing_plan` message (ADR 0397).

**Apply acts only on a proposal.** Apply Now refuses a thread with nothing pending, and never turns unproposed work into a change. A standing apply drops when the settle withholds the work, and waits while unproposed work has no reason yet.

The Diff button follows git separately. It can appear earlier, when the worktree post-commit hook moves the change state from `none` to `unproposed`.

`files` carries the branch's net diff, so the event also corrects an **existing** change. When later commits cancel the diff out, the same `change_id` is re-emitted with `files: []`. That re-syncs the row to zero files and clears `requires_restart`. The change stays `pending`, so the thread stays `proposed`, until the user resolves it. An empty-`files` emit never *creates* a row, so a diffless session never gets an empty change.

**A coding-agent thread holds at most one open change at a time**: pending or set aside. A thread works on one branch and worktree, and a unique index allows one open change per branch. New work on a set-aside change's branch emits `ChangeBroughtBack` and re-proposes under the same `change_id`.

A merge-conflict re-run, or any re-spawn, can propose on a *new* branch. `propose_change` then first discards the thread's open changes on the *old* branches. It emits `ChangeDiscarded` for each **before** the new `ChangeProposed`, so the discard's reset cannot wipe the fresh change state. A `proposed` state follows the pending `changes` rows, so order does not matter for it.

As a backstop, `apply_change` runs the same reconcile after a *successful* apply. It is gated on `ApplyStatus::Applied`, never `Noop`, `Hardening` or `Conflict`, which would discard a newer sibling. That covers the panel Apply, the no-live `apply_now` paths and the Apply-All driver. The live in-place merge path reconciles in `apply_now_success`.

Without this, an orphaned `pending` row lingers on the abandoned branch. The frontend reads *any* pending row as "has pending changes", so it offers Apply and Discard and never Archive. The reconcile keys on branch and `change_id`, so same-branch multi-change survives. `discard_change` notifies the Apply-All driver, so discarding a batch member advances the batch. See `docs/plans/2026-07-01-orphaned-pending-change-blocks-archive.md`.

**A proposal never takes a thread off an open question.** A thread parked on an unanswered `UserQuestionAsked` keeps `waiting_for_user_answer` through a `ChangeProposed`. So recovery after a restart never offers Apply over the question. Apply Now refuses a parked thread with a 409 and the `question_open` reason, or `question_unknown` when it cannot check. Every other Apply route refuses a parked thread whatever its status reads. See ADR 0293.

Legacy: events with an empty `change_id` and `commit_sha` set come from the old per-commit git hook. It was deleted with `commit_hook.rs` and `/api/v1/internal/commit-made`, to enforce "never auto-propose for unfinished work". On replay these events update an existing pending row by branch, and never insert one. The thread's change state and its `requires_restart` follow that row, as they follow every `changes` write.

The current post-commit hook emits no `ChangeProposed`. It only moves the change state between `none` and `unproposed`, through the internal `coding-agent-diff-refresh` endpoint. The `changes` table is a projection over the aggregate events only.

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `ChangeProposed` | End-of-turn aggregate emit from `propose_change` (the coding agent finished a turn `Generated` with worktree changes, and the proposal hold cleared the branch: a satisfying plan marker, and `/harden` run on any work beyond plan files). Carries `change_id` (non-empty UUID), optional `description`, `files`, `requires_restart`, optional `origin`, `commit_sha: None`, `branch_name`, `repo_root`, `hardened`, `incomplete`, plus legacy `path` / `diff` for old rows. A pending proposal is never incomplete: an unfinished turn emits `ProposalWithheld` instead (ADR 0400). `incomplete: true` now marks only a set-aside proposal from the archive net, which cannot tell whether the turn finished. Older rows can carry it on a pending proposal from a Stop. `set_aside: true` proposes straight into set-aside and leaves an archived thread archived. The engine does that for work on an archived thread's branch, with origin reason `archived_branch_work`. It runs once per branch, on the archive or a boot pass, and never once a change row names the branch. A pending proposal omits `set_aside`, so a trigger wanting only applicable changes filters `{"set_aside": {"$ne": true}}`. The legacy per-commit shape (empty `change_id`, `commit_sha` set) is inert in the projection. | per-action | yes | yes |
| `ChangeApplied` | A change was merged to main. Carries `change_id`, `requires_restart`, `client_update`, `commits: Vec<String>` (subjects, oldest first), optional `thread_title`, optional `actor`, optional `pre_merge_sha` / `post_merge_sha` (used by Revert), legacy `path`. | lifecycle | yes | yes |
| `ChangeDiscarded` | A pending change was discarded. Carries `change_id`, optional `actor`, legacy `path`. | lifecycle | yes | yes |
| `ChangeReverted` | An applied change was reverted. Carries `change_id`, optional `actor`, legacy `path`. | lifecycle | yes | yes |
| `ChangeSetAside` | A pending change was kept for later, out of Review, attention and Apply All (ADR 0328). Carries `change_id`; the actor is on `EventMeta`. Its thread can be archived. | lifecycle | yes | yes |
| `ChangeBroughtBack` | A set-aside change returned to pending: the user brought it back, or its thread proposed new work on the same branch. Carries `change_id`; the actor is on `EventMeta`. An incomplete set-aside row never returns to pending: Bring back emits `ChangeWithdrawn` for it instead. | lifecycle | yes | yes |
| `ProposalWithheld` | A turn end left work on the branch and proposed none of it. The plan floor held it, the work never ran `/harden`, or the turn did not finish (a Stop, a failure, an abort). Carries `branch_name`, `files` (what a proposal would have carried) and `reason`: `plan_missing`, `plan_awaiting_approval`, `outside_bound`, `hardening_missing` or `turn_incomplete`. Sets the thread's change state to `unproposed` with that reason, and leaves a `proposed` state alone. Two boot sweeps emit it too. The held-back sweep emits only when the reason differs from the projected one, so restarts add no duplicates. The sweep that withdraws an old pending incomplete change emits it after the `ChangeWithdrawn`. Moves no section. | per-action | yes | yes |
| `ChangeWithdrawn` | A pending or set-aside change went back to unproposed work. The row's status becomes `withdrawn`, and the branch keeps every commit. Four causes: an unfinished turn moved the branch past a pending or set-aside change; a session running at shutdown had a pending change; Bring back met an incomplete set-aside row; or the first boot after ADR 0400 met an old pending incomplete row. Carries `change_id`. Moves the thread to the inbox. After a Bring back, the engine decides again: it proposes under a fresh id if the last turn finished, and otherwise emits `ProposalWithheld`. | lifecycle (rare) | yes | yes |
| `ChangeApplyFailed` | Apply attempt failed mid-merge. Carries `change_id`, `error`, optional `actor`. | lifecycle | yes | yes |
| `ChangeHardened` | The change's working tree was hardened (`/harden` marker stamped on HEAD). Idempotent: the projection reads only the latest event per `change_id`. A fresh `ChangeProposed` with `hardened: false` downgrades it. | lifecycle | yes | yes |
| `ChangeSummarized` | A model wrote the *change summary* for a change of several commits. Carries `change_id`, `summary` (one line) and `description` (the commit list it summarized). The projection stores the summary only while that list is still the change's description. A `ChangeProposed` with a new description clears it. | per-action | yes | yes |
| `MergeConflictDetected` | The engine hit a merge conflict pulling main into a coding-agent branch. Carries `change_id`, `files`, optional engine-stamped `origin`. Also the open half of the *conflict-resolution duty* pairing. While it is a pending change's latest merge-lifecycle event, a conflict resolution is in flight. Closing events: `MergeResolutionCleared` / `ChangeApplyFailed` / `ChangeApplied` / `ChangeDiscarded`. Any recovery-shaped continuation then re-attaches the duty, so the resumed session finishes the apply (see `ContinuationRequested`). | lifecycle (rare) | yes | yes |
| `MergeResolutionStarted` | A merge-resolution worktree was set up. Carries `change_id`, `worktree_path`, `temp_branch`. Survives restart so startup cleanup can find dangling worktrees. | lifecycle (rare) | yes | yes |
| `MergeResolutionCleared` | The merge-resolution worktree was torn down (cleanup finished). Carries `change_id`. | lifecycle (rare) | yes | yes |

## Cross-thread / context

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `ChildThreadCompleted` | A child thread spawned by `run_thread` / `run_coding_agent` reached a terminal event. Coding agent: `CodingAgentIdled` or `SessionEnded`. Chat: `ResponseGenerated` / `ResponseFailed`. Either: a restart's `ResponseAborted` that nothing will resume, reported as `interrupted`. Emitted on the **parent** thread by EventBus fan-in, so the row's `thread_id` is the PARENT. Fires once per completed TURN, so a child that was followed up on (or continued) reports again. Carries `child_thread_id`, optional `child_thread_title`, `status: ChildCompletionStatus` (snake_case: `success` / `failure` / `no_changes` / `canceled` / `interrupted`), and `summary` (truncated to 2000 chars; indexed by `indexable_text`). Also `pending_change_ids` (the child's own branch) and `sub_thread_pending_changes` (every pending change in the child's sub-threads at any depth). Each sub-thread entry is `{ change_id, thread_id, thread_title?, thread_unsettled }`. Both lists are omitted when empty. Queryable by an app via `lucidos.events.query({ event_type: 'ChildThreadCompleted' })`, see § "One table, two enums". | per-action | yes | yes |
| `ChildThreadStopped` | A user Stop ended a child thread's turn (Cancel on its question card included), so the child is now a *stopped child*. Emitted on the **parent** by the same fan-in, in place of a `ChildThreadCompleted`. It wakes nothing and runs no parent turn: the child is alive, and the parent is still owed the settling `ChildThreadCompleted`. Carries `child_thread_id` and optional `child_thread_title`. See § `ChildThreadStopped`. | per-action | yes | yes |
| `ChildThreadDetached` | A child thread was moved to top level (the thread menu's **Move to top level**, the `threads` tool's `detach_child`, or `lucidos threads detach`). Emitted on the **former parent**, never on the child. The projection cuts the edge: the child's `parent_thread_id` becomes null. It wakes nothing, and the child keeps running. Carries `child_thread_id` and optional `child_thread_title`. See § `ChildThreadDetached`. | per-action | yes | yes |
| `ContextDismissed` | **Retired by ADR 0109 and still readable.** Nothing emits it: `dismiss_from_context` is gone, because under *self-curated context mode* the *swept window* takes a result on its own. The resume helper still honours existing rows, so a body an agent dropped earlier stays dropped. Carries `dismissed_event_id`, the *handle* of the event the body came from. | per-action | yes | yes |
| `ContextKeptOpen` | The agent reset one tool result's clock to zero by writing its address under `[KEEP OPEN]` in its *working understanding*. Carries `kept_open_event_id`, the *handle* of the `ToolCalled` behind the result. Same-thread only, and only that type, since a keep moves the clock on a `tool_result` block. The keep is applied where the span is parsed, so this event is the durable record, not the mechanism. It applies once, from the reply that wrote it. It exempts the item from no pass: the trimmer at the wall takes held items last, but still takes them. Only a workspace in *self-curated context mode* sees it, since nothing is swept elsewhere. | per-action | yes | yes |
| `WorktreeCleaned` | Worktree cleanup ran on this thread: the background worker, a Disk Usage row button, or the page's recommended cleanup. Carries `tier: u8`, `freed_bytes: u64` (best-effort) and `branch_deleted: bool`. Tiers: 0 = a *finished worktree* removed (by the worker after the short grace, or by the recommended cleanup at once); 1 = build artifacts stripped, worktree kept; 2 = entire worktree removed, also used for *stranded* worktrees whose git admin dir is gone. `branch_deleted` marks a full removal that also dropped a fully-merged branch; always false for a stranded removal. | lifecycle (rare per thread) | yes | yes |

## Event wait (a thread holding a subscription)

The lifecycle of one `await_event` call. All four are persisted, and all four are
**triggerable but never awaitable** (see § "Triggerable is not the same question
as awaitable"). There is no `thread_event_waits` table: `EventWaitStarted` *is*
the wait, and the dispatcher rebuilds its live set from these rows at boot.

| Event | When it fires | Volume | Persisted | Triggerable | Awaitable |
|---|---|---|---|---|---|
| `EventWaitStarted` | A thread registered a subscription. Emitted between the `ToolCalled` and its `ToolResult`, so the pair closes normally and the turn carries on. Carries `wait_id`, `tool_use_id`, `on: EventSubscription[]` (same shape as a trigger's `on:`), `reason` (the model's own words, shown to the user), `armed_at`, `expires_at`, and `watermark`. The watermark is the event `sequence` at registration, where the catch-up scan starts. `armed_at` is recorded, not derived from `expires_at`, so the age `list_event_waits` reports never drifts. Rows from before 2026-08-07 lack it and fall back to the row's own `created`. Writes NO status. | per-action (rare) | yes | yes | **no** |
| `EventWaitDelivered` | A matching event resolved the wait. Carries `wait_id`, the matched `event_id` / `event_type` / `payload` (self-contained, so replay never dangles), and `matched_index` (which `on:` entry fired). | per-action (rare) | yes | yes | **no** |
| `EventWaitExpired` | The wait passed `expires_at`. **Re-opens the thread** with an explanatory message, because a silently dropped wait is a permanently stalled thread. Carries `wait_id`. | per-action (rare) | yes | yes | **no** |
| `EventWaitCanceled` | The subscription was stopped before its own resolution. `cause` is one of `user_stop` (the **Stop waiting** button), `agent_stand_down` (the agent retired one of its own, or stopped the background task it watched, see `BackgroundBashCompleted`), `thread_archived`, `thread_discarded`. Neither an ordinary user message nor a thread-level **Stop** touches a subscription. `thread_canceled` is a RETIRED cause, still read so pre-2026-08-07 rows replay, never emitted. Also carries `on` and `reason`, a copy of what was stopped. So the transcript entry stands alone on replay, like a delivery, even when its `EventWaitStarted` is outside the loaded window. Both are absent on pre-2026-08-07 rows. Because a cancel re-opens nothing, it is when `todo_consumer` settles a parked *todo list*, unless a turn still owns it (see `TodoListWritten`). | per-action (rare) | yes | yes | **no** |

### A subscription does not hold the turn

`await_event` returns immediately, like any other tool. The turn ends with an
ordinary terminator, and the thread is then plain **`idle`** while it watches:
no queue slot and no blocking state. Archive stays offered, and archiving
cancels the subscription. The per-thread **waiting indicator** shows a live
subscription, not the thread status.

**One thing does wait with you: a change you already proposed.** The thread
reads *Waiting* rather than *Changes to review*, and Apply and Discard are
withheld until nothing is left to wake it. You may commit again after the
delivery, so applying now would merge a branch still in progress. The user's
way out is **Stop waiting**, which ends the subscription and returns both
buttons. A turn that parks with a change pending tells the user "not yet".

**A sub-thread you are waiting on does not hold your change.** It works in its
own worktree and never writes your branch, and your committed work is whole at
each turn's end. So a thread idle apart from running children reads *Changes
to review*, and the user can Apply or Discard. Work you commit after a child's
completion wakes you comes back as a new change. An apply clears your branch's
plan and harden markers, so a Lucidos-source thread plans and hardens that new
round again. So never hand your own change to a "carrier" child.

#### It already happened: the registration result may hand you the answer

A subscription watches **forward only**, so it never fires for something
already past. If the thing might be in the past, check state first. A wait for
an event that already happened just idles until the timeout.

You need not worry about the **race between that check and the call**. If a
match landed in the few minutes before it, registration names it in the
`await_event` result, with its payload and its age.

**That is a report, not a delivery.** The subscription will never fire for
anything the result names. A turn that ends without acting on it leaves the
thing unhandled. Only you can tell an event you missed from one you handled
earlier in the same turn. Act on it now, or say that you already did.

Only an event an earlier wait handed you (an `EventWaitDelivered`) suppresses
that report. So a re-arm right after a **delivery** is never told about the
event it was just handed.

That covers a delivery, not every re-entry. A *child-completion* callback also
re-opens a thread, and the fan-in writes no `EventWaitDelivered`. A re-arm on
`ChildThreadCompleted` in that turn can name the very callback that re-opened
you; recognise it by `child_thread_id` and age, and carry on. Better, never
subscribe to your own child: the `ChildThreadCompleted` section explains why.

#### Which resolutions leave you still watching, and which do not

| Wake | Subscription after it | What to do |
|---|---|---|
| **Delivery** (`EventWaitDelivered`) | **Spent.** The first match resolves the wait and consumes it. Any *other* live wait on the thread is untouched. | To catch the next one, call `await_event` again *before the turn ends*. Saying you will re-subscribe is not re-subscribing: a turn that ends with no new call leaves nothing watching. |
| **Expiry** (`EventWaitExpired`) | Gone. | Report what you were waiting for, rather than subscribing again to the same thing. |
| **Stopped** (`EventWaitCanceled`) | Gone, because somebody stopped it: the **Stop waiting** button, an archive or discard, or you standing it down. There is no re-entry, so the thread is left as it was. | Report back. Do not re-register unless they ask. |

Delivery is the one that bites. It alone consumes the subscription *and* hands
you a payload, so it reads like the wait is still running. A standing in-thread
watch is one subscription per event, bounded by the recent-subscription cap in
§ "Limits": past it, `await_event` is refused and you report back. Never promise
the user "forever" in a thread; that is a trigger's job.

A **user message** and a thread-level **Stop** are absent from the table on
purpose. Neither resolves anything, so every subscription survives with its
deadline intact. Stop ends the running turn, which no subscription was holding.

**No `EventWait*` event writes a status**, by rule. Registration happens
mid-turn, so the turn's own terminator decides. A resolution lands on an idle
thread, whose own `PromptInjected` sets `running`, or on one running
unrelated work. A status write there would misreport it as revived.

Waits were once **attached** to an unpaired `tool_use`. That shape is gone: see
`docs/plans/2026-08-06-every-event-wait-is-detached.md`.

### The re-entry anchor

Every delivery and every expiry is followed by exactly one more event that
carries the payload as prose: a **`PromptInjected`**. It starts a new
exchange, the honest shape for something that may arrive hours later. A
child-thread completion re-opens its parent the same way.

**It says the event arrived, never that you were asleep.** Registration does not
hold your turn, so a match can land while this thread is still working. The
engine then folds it into the running turn and says it arrived "while you
were working". The transcript card marks it `arrived` for the same reason: a
delivery does not know which of the two lanes it took.

On a *delivery* it also carries `delivered_event_id`, the id of the
`EventWaitDelivered` above it. The prose is the model's prompt and cannot be
trimmed, but rendered verbatim it is a screen of JSON. So the transcript reads
`event_type` and `payload` from that row and names the event with its payload
folded away. An *expiry* leaves the field unset, having no payload.

Both carry `origin: { kind: "engine", reason: { kind: "event_wait", outcome,
watched, wait_reason } }`, with `mode: "agent"`. `outcome` is `delivered` or
`expired`. `watched` names each event type the wait watched, once. `wait_reason`
is the reason the agent gave when it subscribed. The transcript reads these to
say who wrote the prompt and why. Older rows carry no `origin`.

On restart this matters: a resolution followed *only* by its anchor is one whose
turn never ran. That is how the engine re-drives a re-entry lost to a crash.

### Both agents, one registration

The chat agent registers through the `await_event` LLM tool. A **coding agent**
uses `lucidos await-event` (see `lucidos-cli`), which POSTs
`/api/v1/threads/<id>/event-waits` into the same code. So the caps, the
subscribability gate and the refusal wording are one implementation. The
delivery routes down the coding-agent lane (into a live session, or a fresh
resume when there is none), exactly as a child completion does.

### Limits

Three, all refused at registration with an error the agent reads in the same
turn:

- `timeout_secs` is **required** and capped at **24 hours**. There is no
  unbounded wait. Anything longer is a trigger.
- A thread may hold **25 live waits** at once, and may not register the same
  `on:` list twice (one event would then deliver twice). The limit is on
  outstanding re-entries, not on what you watch. One wait's `on:` list is
  uncapped, so a dozen things in one subscription (any entry delivers) costs one
  of the 25.
- A thread may start **20 counted waits within an hour** with no message or
  question-card answer from the user in between. **A wait another thread's event
  ended does not count**, so waiting on other threads one at a time never
  reaches it. Everything else counts:
  - a wait ended by this thread's own event;
  - a wait ended by an event with no known source (`emit_event` writes those);
  - a wait that timed out, and one not yet ended.

  That bounds a thread that re-opens itself and a model stuck re-arming. An
  agent- or engine-authored message does not reset the count. Engine-armed
  waits for this thread's own background tasks count too.

## Voice session (a thread being spoken to)

The lifecycle of one *voice session*. Voice is a **mode of a chat thread**, never
a kind of thread (ADR 0148). So a session leaves the thread's `source` at `chat`,
opens no `channel` of its own, and moves neither status nor section: a live
microphone is not a turn.

Voice is **experimental and off by default**. None of these events can occur
until a workspace sets the `voice_enabled` preference. A session runs on the
*home thread* only (ADR 0362), so these events land on no other thread.

There is no voice-session table. The two lifecycle rows below *are* the session,
which is what lets the boot sweep find one whose engine died mid-call.

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `VoiceSessionStarted` | A voice session opened on this thread. Carries `session_id: Uuid`; the `actor` (from `EventMeta`) is the device that opened the socket. One session may be live per thread, so a second upgrade is refused and writes no row. Placing a call bumps the thread's recency and nothing else. It does NOT promote a draft: connecting is not a conversation, so the first spoken word does that (ADR 0167). | lifecycle (rare per thread) | yes | yes |
| `VoiceSessionEnded` | The session closed. Carries the same `session_id`, `duration_secs: u64`, and `reason`. Reasons: `hangup` (the caller rang off); `agent_hangup` (the caller said they were done and Lucidos rang off, ending the call but never the work); `disconnected` (the socket died with no goodbye); `provider_failed` (the talker could not go on); `engine_shutdown` (the engine went away, or the boot sweep settled a start its process never ended). A sweep-settled row carries `duration_secs: 0`, since the engine holding the clock is gone. | lifecycle (rare per thread) | yes | yes |
| `SpokenReplyGenerated` | What the talker said in one finished turn of speech. Carries `session_id: Uuid`, `text: String`, `interrupted: bool` (the caller spoke over it, so only that much was heard) and an optional `spoken_secs_before: f64`. Written when the provider ends that turn, so `created` IS when the words stopped (ADR 0201). `spoken_secs_before` is how long the talker had been speaking by then. The transcript places the row at `created` minus it, where the talker BEGAN, before every step the words were said over (ADR 0206). It is absent on a reply that streamed no deltas, which reads at `created`. One per provider TURN, whether the talker composed the words or read the agent's answer aloud. A transcriber cuts sentences where the speaker breathes, so the transcript and the agent's history join neighbouring rows back together. A reply the caller CUT OFF carries only what they heard; the later full-turn report writes nothing (ADR 0200). A reply cut off before any word writes nothing. The `actor` names the talker as a guest agent, so the agent sees it under the talker's label, not as its own prior turn (ADR 0150). It is `Metadata`, because the agent's turn owns the thread's status. Like the spoken message, it MAKES THE THREAD REAL (ADR 0167), which matters because the talker usually greets first. | a few per call | yes | yes |
| `SpokenMessageReceived` | The caller said something on a call: EVERY caller utterance, whatever the talker does with it (ADR 0201). Carries `session_id: Uuid` and `text: String`; the `actor` is the caller's device. Written when the provider ends the caller's turn, so `created` is when they stopped speaking. It starts no agent turn, which is why it is not a `MessageReceived`: that Start event would claim a turn that never runs. The talker's `WorkDelegated` starts a delegated turn. `Metadata`, and it moves no section. It MAKES THE THREAD REAL (ADR 0167): a draft the call was placed from becomes an ordinary thread, its stored draft is cleared, and every device is told. The caller's FIRST spoken words become the thread's `first_message`. They never retitle Home, which only the user names (ADR 0362). | a few per call | yes | yes |
| `WorkDelegated` | The talker asked for the agent with its `delegate` tool. Carries `session_id: Uuid` and `reason: String`, the talker's few words on what the caller wants. It is empty when the talker composed none, as on any protocol whose delegation frame carries no words. The transcript then draws no row, rather than putting the caller's sentence in the talker's mouth (ADR 0200). The `actor` names the talker as a guest agent. **`Start`: this row begins a delegated call's turn** (ADR 0201), and the turn anchors on it. No `MessageReceived` is written beside it, since the caller's words have their own row. | a few per call | yes | yes |

No payload here carries audio, and audio is never persisted.

**The talker decides whether a spoken turn needs the agent.** It holds exactly
three tools and none of them acts (ADR 0170). `delegate` takes a short reason
and writes `WorkDelegated`. THAT row starts the agent's turn, through the same
single-flight admission a typed message uses (ADR 0201).

`answer` settles something waiting on the caller: a question card, or a
permission card in any of its three lanes. It hands back a choice id the engine
issued, so nothing matches a spoken word against a label. The resulting
`UserQuestionAnswered` or `*PermissionResolved` is the row the screen writes,
bar its actor. A `delegate` is refused while one of those is open, because the
agent is blocked inside it.

`hang_up` ends the CALL when the caller says they are done, never the work, so
a turn in flight keeps running. It writes `VoiceSessionEnded` with reason
`agent_hangup`.

Each utterance is recorded exactly once, so a call's transcript is the
thread's transcript: the caller's words, the agent's answer where there was
one, and the `SpokenReplyGenerated` rows for what was said aloud.

**Every row lands when its own turn ends** (ADR 0201). So nothing finished
lives only in engine memory, and the caller's words read above the reply to
them.

**The tool is an ask, not a wake.** The talker never knows whether an agent
turn is running. It calls `delegate` for every request needing the agent, even
while earlier work is going. Single-flight admission decides whether that
starts a turn or joins one.

**What the talker says is not what the doer wrote.** The written answer is
handed over to be SAID in the talker's own words, never read out. Past 400
characters the talker gives the headline and asks whether the caller wants the
detail. The full text reaches the talker either way, so a "yes, go on" is
answered from it, not invented. The thread keeps the full text.

A session's spend lands as one `ContextCaptured` per spoken reply, with
`purpose: "voice"`. So a cost rollup reads voice like every other model call.

## Which call each purpose names

Every `ContextCaptured` row says which model call it records. `turn` is an
agent's own round trip, and an absent `purpose` (every row from before the
field) means `turn`. The other eighteen are *auxiliary model calls*: ones the
engine makes for itself, and ones an app or script makes through the proxy.
**Every model call the engine makes carries a purpose** (ADR 0242). Only the
*model call service* can call a model, so a call that records nothing does
not compile.

**Every row lands on a thread.** A call made for a thread records there. A
call no thread caused records on the workspace's *home thread*, which every
workspace has.

An auxiliary row carries `producer: "auxiliary"`, no `tools`, a
`context_window` of 0 (a single-shot call has no budget) and one body-less
section sized to the request. It is recorded per ATTEMPT, so a resampled title
or retried extraction leaves several rows, since each spent tokens. The
transcript never renders one.

Each auxiliary model preference has one purpose (ADR 0107). A purpose the user
can point at a model owns its own `model_*` / `reasoning_*` pair. One that
cannot says why: a fixed backend, or the agent's own chat model.

| `purpose` | The call | Model |
|---|---|---|
| `turn` | An agent's own round trip. The only one with a section breakdown. | the thread's |
| `title` | The thread title, resampled up to twice. | `model_title` |
| `change_summary` | The one-line *change summary* for a change of several commits. | `model_change_summary` |
| `image_describe` | A caption for an image the user sent. | `model_image_description` |
| `memory` | Fact extraction over one message. | `model_memory` |
| `conversation_summary` | The paragraph standing in for a thread's older assistant turns. | `model_conversation_summary` |
| `summary_compaction` | One round of the compactor writing a summary line for the Tree memory module. Recorded on the thread the line summarizes, or on the thread that wrote an artifact it summarizes, else on the home thread. | `model_summary_compaction` |
| `memory_find` | One batch of the recall tool's `find`, judging about 40 summary lines against a query. Recorded on the thread that asked; a `find` from the API or CLI with no `--thread` records on the home thread. | `model_memory_find` |
| `query_classification` | The yes/no questions in front of memory retrieval. | `model_query_classification` |
| `read_decision` | The turn-end gate's forced *read decision*, for a turn that ended without one. Recorded on that thread. | `model_read_decision` |
| `image_gen` | One `generate_image` call. | `image_model` |
| `voice` | One spoken reply from a *voice session*'s talker. | `model_voice_talker` |
| `command_judge` | The *command guard*'s judge over one ambiguous command, on either backend. | `model_command_judge` |
| `judge_tool` | One call of the agent's own `judge` tool. | fixed: TypeSafe |
| `intent_loop` | One round of an `execute_intent` sub-loop. | the agent's own |
| `memory_correction` | The verdict behind `correct_memory`, over which entries to delete. | the agent's own |
| `artifact_summary` | The summary written for a file the `import_file` tool imported. | the agent's own |
| `side_question` | One *side question*: a Lucidos Agent round, or a Claude Code session copy's whole run. | the agent's own |
| `web_search` | One `web_search` call, on whichever backend answered. | fixed per backend |
| `proxy` | One model call an app or script made through the credentialed proxy (`/api/v1/proxy/...`, `lucidos proxy`). Recorded on the thread whose subprocess made it, else on the home thread. Only a reply carrying a usage block records. | the caller's |

`intent_loop`, `memory_correction`, `artifact_summary` and `side_question` run
on the agent's own chat model. They own no preference, so Settings offers no
row for them. `judge_tool` and `web_search` own none either, for a different
reason: their backend fixes the model. `proxy` owns none because the caller
names the model in its own request.

## Transient: never persisted, broadcast over SSE only

Transient names are past tense too. They cannot trigger, since the matcher sees only persisted events. They drive live UI state (streaming preview, in-app refreshes, navigation) and parent-thread fan-out signals. A request the user must answer is never transient: it is a persisted *form request* (§ Form requests), so a lost stream frame cannot lose it.

| Event | When it fires | Volume |
|---|---|---|
| `CumulativeTextUpdated` | One snapshot of the assistant's streaming buffer (cumulative text so far). Emitted at every flush boundary beside the persisted delta in `TextStreamed`; the frontend overwrites with the latest. Legacy alias: `TextStreaming`. | high-volume-streaming |
| `LlmCallRetried` | The chat agentic loop is retrying an LLM call (rate limit, transient API error, "retry with different approach" path). Carries `reason: String`. Legacy alias: `Retrying`. | per-action |
| `PreambleCompleted` | Reserved: defined on the enum and skipped by the projection's transient match arm, but **not emitted** by any production code path. Legacy alias: `PreambleCompleting`. | n/a |
| `PushNotificationRequested` | Request event that prompts the device to register for web push. Empty payload. Legacy alias: `PushNotificationRequest`. | lifecycle |
| `AppUiRefreshRequested` | Tells an open app iframe with `app_id` to reload itself. From `refresh_app` it carries the turn's *last used device* as its `actor`, and only that page reloads. The end-of-turn refresh carries none, so every page with the app open reloads. Legacy alias: `RefreshAppUI`. | per-action |
| `AppUiCaptureRequested` | Asks an open app iframe to capture state for `request_id`. It carries the turn's *last used device* as its `actor`, as `NavigationRequested` does, and only that page answers. The reply lands via `POST /api/v1/app-capture`, which refuses an answer from another device with 409. With no last used device, there is no actor and any page may answer. `save_format` (`png`, `jpeg` or `webp`, absent by default) asks for a sharp picture to save as a file. Legacy alias: `CaptureAppUI`. | per-action |
| `NavigationRequested` | Tells the frontend to navigate (URL, intra-app route, etc.). Carries `payload: String`. An agent navigate (`navigate_ui`) also carries an optional device `actor`: the device in the tool's `device` argument, else the turn's *last used device*. Every page drops the event unless the actor is its own device. So exactly one device acts: it opens the target if it shows the thread, else it shows a toast the user taps. The actor is absent for a turn with no device and for the SDK app-iframe (nil-thread) path. Only a connected page receives it; nothing stores, retries or confirms it. So the `navigate_ui` result says whether the target device had Lucidos visible in the last 2 minutes. If not, it offers `send_notification` with a navigate tap, which stays in the inbox. | per-action |
| `CodingAgentThreadSpawned` | A child coding-agent thread (spawned via `run_coding_agent` / `run_thread`) started. Carries `cc_thread_id`, `title`, `agent`. SSE-only: the child's persisted record is its own thread row. Alias: `CcThreadSpawned`. | per-action |
| `CodingAgentDiffChanged` | A coding-agent worktree post-commit hook found the branch's diff and moved the change state between `none` and `unproposed`. Carries `has_diff` (the git fact) and a full thread aggregate, so the frontend shows or hides the Diff button at once. Does **not** imply `ChangeProposed` / Apply readiness. | per-action |
| `ChildrenCountChanged` | A parent or ancestor thread's aggregate metadata changed. Carries the full updated aggregate (`active_children_count`, `total_children_count`, `blocking_descendant_count`, `attention_descendant_count`, …). Fires when (a) a direct child terminates and the parent's counts shift, or (b) any descendant's "blocking" or "attention-needing" predicate flips. Those predicates are Running, WaitingForUserAnswer, or `has_pending_changes` && CodingAgent (see `is_blocking` / `is_attention_needing`). In case (b) every ancestor on the chain gets its updated counts. Drives the "Active children" badge, the blocked Archive in the thread menu (`blocking_descendant_count`), the recount Archive runs on a thread already archived, and the Current-bubble routing in `display_section` (`attention_descendant_count`). | per-action |

## Indexable text

`ThreadEvent::indexable_text()` returns the text the memory store indexes. Only the types in `ThreadEvent::INDEXABLE_EVENT_TYPES` return any, and a memory rebuild loads only those rows. The types:

- `MessageReceived`.
- `SpokenMessageReceived`: what the caller said on a call.
- `PromptInjected`, only without `injected_message_id`. With one, it echoes a `MessageReceived` that memory already holds.
- `ResponseGenerated`, `ResponseCanceled` and `ResponseAborted`.
- `ChildThreadCompleted`: its `summary`.
- `ImageDescribed`: its `description`, the only text an image-only turn leaves.

Every other type returns `None`.

## Concrete payload shapes (selected)

`CodingAgentIdled`, `UserQuestionAsked`, `UserQuestionAnswered` and the `CodingAgentPermission*` pair are in `system-knowhow/coding-agent-events.md`. Below are the most-asked chat, lifecycle and change variants.

### `MessageReceived`

```json
{
  "type": "MessageReceived",
  "data": {
    "text": "Summarize my open PRs.",
    "user_image_hashes": [],
    "device_id": "device-abc123",
    "parent_thread_id": null,
    "spawning_event_id": null,
    "mode": "human",
    "model": "claude-opus-4-7",
    "reasoning_effort": null,
    "provider": "anthropic",
    "origin": {
      "kind": "device",
      "device_id": "device-abc123"
    }
  }
}
```

`mode` is `ActorMode` (`human` / `agent` / `engine`). `origin` is the structured `MessageOrigin` (`Device` / `Api` / `Workspace` / `ThreadLink` / `Engine` / `System`). Old DB rows may lack `origin`; the frontend's `legacyOrigin()` synthesizes it from `device_id` / `parent_thread_id`.

**A device is stored by id only.** A `Device` origin or actor carries `device_id` and no name, and `MessageReceived` carries no `device` name. Lucidos shows the device's current name, so a rename reaches older events. Older rows carry a `label` (and `device`) that nothing reads. An app gets the id and no name: no SDK call lists devices today.

`voice_session_id` names the *voice session* a message was **spoken** on. No new `MessageReceived` carries one, since a caller's words are a `SpokenMessageReceived` (ADR 0201). Older rows still carry it, and the transcript reads it to mark such a bubble as spoken.

The `Api` variant carries an optional `source_thread_id`:

```json
"origin": {
  "kind": "api",
  "user_agent": "curl/8.7.1",
  "mode": "agent",
  "source_thread_id": "9c1f-..."
}
```

Set when the engine recognises a Lucidos-spawned subprocess (coding-agent session, `run_bash`, `run_python`, scheduled script, `lucidos` CLI). Detection uses the **thread-bound origin token**. Every spawned subprocess gets its own `LUCIDOS_AGENT_ORIGIN_TOKEN` in its env, shaped `<thread-id>@<depth>@<trigger>.<mac>` under a per-engine-startup HMAC secret. A `-` stands in for any field the spawn does not carry. The `lucidos` CLI forwards the token as the `x-lucidos-agent-origin-token` header on every engine call. The Python shim does the same for urllib and requests.

The MAC covers the whole prefix, so all three fields are authenticated, not claimed. A subprocess can present only the token it was handed. The depth is the event-trigger chain depth (ADR 0138), and the trigger is the fire this subprocess belongs to (ADR 0137). Mutating HTTP handlers (`apply_change`, `revert_change`, `discard_change`, `chat_submit`, settings writes, …) then stamp `Api { mode: "agent", source_thread_id: <spawning thread> }`, whatever the body claims. So agent actions never appear as "You" cards.

A token that does not verify, including a valid one re-pointed at another thread, counts as no subprocess at all. It falls through to the unattributed-API-client path that external clients take. A script calling bare `curl` sends no header, so it takes that path too. The old `x-lucidos-source-thread-id` header was unverifiable; it is gone, and nothing reads one.

Cross-thread chat injection from a subprocess is refused with 403 at `chat_submit`. The target must be the caller's own thread, or a not-yet-existing thread whose declared parent is the caller. `api::chat::subprocess_chat_legitimate` has the full allow/deny matrix.

#### `mode` and `origin` are attribution, and an agent may not fabricate them

Together the two fields answer "who authored this turn", and the projection
acts on `mode`. `human` sets the thread's `initiator` to the user and bumps
`last_user_action`, the drawer's recency sort. So an agent-written
`mode: "human"` turn is a record the user cannot tell from their own.

**An agent must never post a message the engine would record as human.** The
one chat entry point, `POST /api/v1/chat/stream` (`api::chat::chat_submit`),
accepts `mode: "human"` from only two callers:

- a caller whose `device_id` resolves in the `devices` table (the user's own
  client, which sends `x-lucidos-device-id` on every mutating request);
- a `caller_workspace` (the cross-workspace contract, where the calling
  workspace vouches for its own human).

Everyone else is an *unattributed caller* and gets 403. See
`api::chat::human_mode_is_attributed`.

So dropping your origin token buys no privilege. A subprocess is held to
`subprocess_chat_legitimate` (which refuses `mode: Human` outright) for an
existing thread and for any `mode: "human"` post, `caller_workspace` or not. Only its `agent` or `engine`
top-thread create skips that check, and the top-thread authority check covers it.

Three related refusals on the same path, all of which write nothing:

- **404** when `thread_id` names no existing thread and the request carries no
  create signal (`new_thread: true`, a `parent_thread_id`, or a
  `caller_workspace`). Otherwise a caller that reached the wrong engine would
  get its threads created there.
- **409** when the request asserted a different workspace than the answering
  engine serves (`x-lucidos-target-workspace`). The body names the actual
  workspace.
- **400** when the body `mode` contradicts the request's credential, e.g. a
  coding agent's origin token sent with `mode: "engine"`. The body names the
  credential's mode; send that one. See `api::chat::origin_agrees_with_mode`.

**A re-post of the same `event_id` runs once.** A client that lost the answer
to its POST may send the same body again. If the engine already accepted that
`event_id`, it answers `200` with the same `event_id` and starts nothing: no
second `MessageReceived`, no second turn. A refused request leaves its
`event_id` free, so a corrected retry runs. See
`api::chat::message_was_already_accepted`.

If no tool covers what you were asked to do, say so. Never hand-roll HTTP to
the engine around it: see `system-knowhow/lucidos-cli.md` § "Never post
to the engine API as the user".

The `ThreadLink` variant answers "who launched this thread", and it is **independent of `parent_thread_id`**:

```json
"origin": {
  "kind": "thread_link",
  "thread_id": "bc98-...",
  "spawning_event_id": "134e-...",
  "mode": "agent",
  "direction": "parent"
}
```

`parent_thread_id` is the *callback linkage*: it makes the launching thread resume when this one finishes, increments its `active_children_count`, and is what the `thread_summaries` projection stores. The origin is *display attribution*: who to name and link in the message route popover. A `relation: "child"` spawn carries both. Two shapes deliberately carry the origin with **no** linkage:

- a *top-thread* (`relation: "top"`), which names its *spawning thread* but reports back to nobody;
- a *child follow-up*, which attributes the message to the parent without re-counting an existing child.

So never infer parent-ness from a `ThreadLink` origin; read `parent_thread_id`. Example: `resolve_attend_mode` walks the origin chain to decide whether a coding-agent permission card can auto-resolve with the root trigger's side-effect grant. It hops only where the linkage exists, so a top spawn asks a human instead of inheriting a grant.

`image_description` on this payload is **deprecated** and survives only on legacy rows from before `ImageDescribed`. New emissions omit it (`Option<String>` with `skip_serializing_if = Option::is_none`). Read the description from `ImageDescribed`, joined by `source_event_id`. The startup backfill emits one `ImageDescribed` per legacy `(source, hash)` pair, so historical rows read the same way.

### `QueuedMessageRemoved`

```json
{
  "type": "QueuedMessageRemoved",
  "data": {
    "removed_message_id": "550e8400-e29b-41d4-a716-446655440000",
    "channel": "chat",
    "actor": {
      "kind": "device",
      "device_id": "device-abc123"
    }
  }
}
```

`removed_message_id` is the event id of the `MessageReceived` the user removed from the queued follow-up list. The original stays in the append-only log. This marker is metadata-only in the thread lifecycle projection, so it bumps no status, section, recency or message count.

The frontend hides the matching exchange only while it has no steps. If a race already attached `PromptInjected` to that message, the exchange stays visible. The chat loop checks this marker before appending injected prompts, so a removed queued prompt never reaches the model.

On a Claude Code thread the engine has already written the message to the agent, so the marker follows the agent's answer. The session asks Claude Code to drop it (`cancel_async_message`) and records this event only once Claude Code confirms. A message the agent already read gets no marker, and `POST /api/v1/chat/queued-message/remove` answers 409 with `reason: "already_read"`.

The marker outlives the engine. It is one of the three the *stranded queued message* predicate reads. So a retraction made while the engine was down still beats the next resume's recovery (ADR 0236).

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

`source_event_id` is the `MessageReceived` this description applies to. `hash` is the sha256 of one attached blob (one entry in `MessageReceived.user_image_hashes`). A multi-image message emits one event per hash, all with identical `description` text, so collapse on `source_event_id`. `model` is the Flash model that wrote the text (`claude-haiku-4-5`, `gemini-2.5-flash`, …). Rows from the one-shot startup backfill say `"backfill"`, since the original model was not recorded.

`ThreadEvent::indexable_text()` surfaces the `description`, so it is **indexed into memory** like a message. Otherwise a "what's this?" plus screenshot turn would leave no memory trace, since the typed text carries none of the image. Coding-agent threads never emit `ImageDescribed` (it runs only in the chat agentic loop), so their image turns index text-only.

### `ConversationSummarized`

```json
{
  "type": "ConversationSummarized",
  "data": {
    "summary": "Worked through the alignment pass on the edit timeline. The off-by-one on the splice boundary was fixed. Export presets were left as they are.",
    "covers_through_event_id": "550e8400-e29b-41d4-a716-446655440000",
    "covered_count": 31,
    "model": "gemini-3.8-flash"
  }
}
```

This event is the whole **cache** for the older-turn summary. There is no
table, so the paragraph survives an engine restart because the event does. The
first success holds: a later summariser failure reuses this paragraph instead
of rendering a bare "(N earlier messages not shown)" line.

`covers_through_event_id` addresses the newest assistant turn the paragraph
covers. `load_chat_history` compares it against the current older segment. It
re-summarises only when the assistant turns past that boundary exceed
`HISTORY_SUMMARY_REFRESH_AFTER`. Otherwise it reuses the paragraph and renders
the uncovered assistant turns compacted, so nothing is silently dropped.

**The refresh runs detached, so this event lands one turn late.** The turn that
finds a refresh owed renders from the cache as it stood. Awaiting the call
would delay that turn's first step for a paragraph only the next turn uses.

**User turns are never summarised** (ADR 0102). They render verbatim in the
older region, so a constraint stated 40 turns ago is still in the prompt word
for word.

**Every row covers its own thread** (ADR 0124). A chat turn reads only its own
events, so no other conversation's content lands under this thread.

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
`ResponseGenerated` is a **benign empty completion**: a clean, model-decided
stop (`end_turn` / Gemini `STOP` / OpenAI `stop` / `completed`) with no text and
no tool calls. The thread completes Idle with no red error, and the frontend
shows a neutral note that the agent finished without writing a reply. The genuine failure shapes (truncation, safety block,
dropped output, unrecognised stop) still emit `ResponseFailed`. The split is
provider-agnostic: see `classify_empty_completion` / `normalize_finish_reason`
in `crates/lucidos-engine/src/engine/agentic_loop/helpers.rs`.

### `ResponseCanceled` / `ResponseAborted` / `ResponseFailed`

```json
{ "type": "ResponseCanceled", "data": { "text": "partial…", "images": [], "model": "claude-opus-4-7", "reasoning_effort": null, "cause": "user_stop" } }
{ "type": "ResponseAborted",  "data": { "text": "",        "images": [], "model": "claude-opus-4-7", "reasoning_effort": null, "cause": "engine_shutdown" } }
{ "type": "ResponseFailed",   "data": { "error": "upstream 503: model overloaded" } }
```

`cause` values:

- `CancelCause`: `user_stop` (Cancel button), `user_action` (Apply / Discard / Archive on a running thread), `superseded_by_followup`, `unknown` (legacy DB rows). `superseded_by_followup` means a follow-up interrupted a mid-turn **Codex** turn. The engine ran the follow-up as the next turn, keeping partial work.
- `AbortCause`: `engine_shutdown`, `safety_net`, `recovery_after_restart`, `process_killed`, `stale_settle`, `session_dropped`, `unknown` (legacy). `session_dropped` means the run future was dropped because its caller was cancelled; the session entry's drop-guard emits it.

On a *coding-agent thread*, `user_stop` is a **resumable turn boundary**, not a terminator. The `Cancel` button uses the backend's native interrupt, so the session stays alive. `CodingAgentIdled` follows (with the `cc_session_id` when available), the branch is kept, and the next message resumes the conversation. `user_action` (Apply / Discard / Archive) DOES terminate, via its own lifecycle event. See `system-knowhow/coding-agent-events.md` § `CodingAgentIdled` "Cancel = Esc".

`superseded_by_followup` is mechanically a cancel, but the user **steered** rather than stopped. There is no `ResponseGenerated` and no proposal for the partial work. The branch is kept, and the follow-up turn's proposal includes both turns' files. The frontend renders it **neutrally**: a plain "Done", with no "Canceled ✕" badge and no "Response canceled" panel. Only Codex produces it: Claude Code steers a mid-turn follow-up via stdin, and idle-Codex follow-ups use `turn/start`. See `docs/plans/2026-06-21-codex-followup-redirect-label.md`.

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

`stdout` / `stderr` are capped at 100 KB each and keep the **tail** when they
overflow, since the end of a build log is where the failure is. A leading
marker opens `[truncated`, then a dash, then
`N earlier bytes dropped, showing the most recent M of T total]` verbatim. `bash_output` renders the same tail when it
falls back to this row, so the live drain and the archive never disagree. `N`
also counts what the engine's ~2 MB per-stream ring buffer discarded, so it is
the real gap.

An *event wait* that delivers this event cuts each stream to its last 4000
bytes in the prompt that re-opens the thread. The full row stays in the event
log, and `bash_output` / `lucidos background-task output` read it.

**Reading the status.** `exit_code` and `signal` are mutually exclusive, and neither ever stands in for a status the engine did not obtain:

| `exit_code` | `signal` | Meaning |
|---|---|---|
| `0` | absent | The command really exited 0. A reader can trust this. |
| non-zero | absent | Normal exit with that status. |
| `null` | set | The child was terminated by that Unix signal: `15` SIGTERM (the watchdog timeout and `bash_kill` send it first), `9` SIGKILL (their follow-up after a 3 s grace, and the teardown), `11` SIGSEGV, `13` SIGPIPE (a pipeline producer whose consumer closed the pipe). |
| `null` | absent | The engine could not determine the status. Treat as failure, never as success. Also the shape of rows written before `signal` existed. |

`signal` is omitted from the payload when there is none. So is `abandoned` when false.

**`abandoned: true` means the engine went away, not that the task failed.** A background task is a child of the engine process. A restart, a crash or an OOM ends it, and nobody is left to reap a status.

The engine records that from two places. Graceful teardown kills each running task, waits for the reap, and writes the completion with all its output. The next boot sweeps up what teardown could not reach: every SIGKILL, OOM, panic and power cut. Both rows carry `abandoned: true`, no `exit_code` and no `signal`.

That keeps the promise in `system-knowhow/running-python.md` that ending a turn with background work running is a valid wait. The subscription resolves at the next boot, not at its own deadline.

**The `stderr` line says which path wrote the row.** Teardown killed the task and kept its output. The boot sweep kept neither, since no destructor ran after the crash.

**Neither promises the work stopped.** A crash kills nothing. The teardown's SIGKILL reaches the task's whole process group, but misses a process that detached into its own session. So either path can leave a child running under init. Check before re-running the same work.

`abandoned` is distinct from `killed`, which means `bash_kill` was called, and outranks it when the engine's own shutdown sent the signal. Reading one as the other says someone called the work off. On the boot path, `finished_at` is when the loss was recorded, so it spans the downtime and is not the task's runtime.

**A failing pipeline stage is never masked by a later succeeding one.** The engine runs commands under `bash -o pipefail`, so `cargo clippy … 2>&1 | tee build.log` reports clippy's `101`, not `tee`'s `0`. Without `pipefail`, failing builds silently reported `exit_code: 0`. `pipefail` reports the *rightmost failing* stage (`sh -c 'exit 42' | sh -c 'exit 7'` → `7`). So it shows that a pipeline failed, not always which stage. On a host with no `bash` the engine falls back to `/bin/sh`, logs `[Shell] no bash found …`, and the guarantee does not hold.

Both events also fire for `run_python_background`. The `command` then carries the venv-rooted python invocation, e.g. `'/<ws>/.lucidos/runtime/python/venv/bin/python' '/<ws>/.lucidos/exhaust/<run_id>/script.py'`. The script stays under `.lucidos/exhaust/`, so the audit trail records which one ran. One registry, one event pair, one watcher: consumers never branch on the spawning tool.

### `ContextCaptured`

```json
{
  "type": "ContextCaptured",
  "data": {
    "producer": "main_llm",
    "model": "claude-opus-4-7",
    "context_window": 200000,
    "sections": [
      { "name": "System Instructions", "budget_delta_chars": 49380, "content_chars": 49380, "role": "system" },
      { "name": "Memory", "budget_delta_chars": 1690, "content_chars": 1690, "role": "user", "group": "Memory & history" }
    ],
    "tools": ["bash", "read", "edit"],
    "estimated_total_tokens": 20428,
    "usage": { "input_tokens": 19878, "output_tokens": 0, "cache_read_tokens": 0, "cache_creation_tokens": 0 },
    "trimmed": false
  }
}
```

`producer` is `ContextProducer`, serialized snake_case: `main_llm`, `claude_code`, `codex`. Match those exact values in a `condition:` filter; the PascalCase Rust names never match. `usage` is `None` pre-call and on a call whose provider reported no prompt tokens. When present it carries the real provider-reported counts. **`usage.modality` splits the four counts across text, audio and image**; only a realtime voice call reports one. Each part is named after the total it sums into, and a producer that cannot fill every field sends none.

A Claude Code sub-agent's call carries `parent_tool_use_id`, the id of the `Agent` call that spawned it. Every other capture omits it.

A Claude Code capture also carries `api_call_id`, the agent's id for that API call. Every `CodingAgentToolCalled` the call produced carries the same value, so group on it to find the tool calls one model call made. Codex captures and older rows omit it.

A `reconstructed: true` row was rebuilt by the startup backfill from events never captured live. Its `estimated_total_tokens` is a reconstruction and it carries no `usage`, so a rollup filtering on `usage` still reports measured spend only.

`reasoning_effort` names the tier an auxiliary call ran at (`low`, `medium`, …), where the caller reports one. Only the Tree memory compactor sets it today, so a cost estimate can split its measured usage by tier instead of one undifferentiated baseline. Absent on every other purpose, and on every row written before the field existed.

`served_model` is the model id the provider's reply named, verbatim: Gemini's `modelVersion`, or `model` from OpenAI, Anthropic and OpenRouter. `model` is the one Lucidos asked for. The two differ when another model answered: a provider routed the id elsewhere, or Lucidos sent a retired model's successor. A dated snapshot of the same id (`claude-haiku-4-5-20251001`) is not a reroute. Absent when the reply named no model, on a streamed proxy reply, and on every row written before the field existed. A coding-agent row puts the agent's reported model in `model` and omits this field.

`duration_ms` is how long an auxiliary call took, wall clock, where the caller timed it. Only the Tree memory compactor sets it today, so the backfill estimate can use a model's measured seconds per call at each tier. Absent on every other purpose, and on every row written before the field existed.

`context_window` is the model's window in tokens as the engine resolved it. That is the value on the model's *model registry* row, or else a guess from the model id (`[1m]`→1M, `claude-`→200k, `gpt-5`→400k, else 200k). A capture showing `200000` for a bigger model means its row declares no *context window*. The turn was then budgeted against the smaller number.

`estimated_total_tokens` covers the system prompt, the tool definitions and the messages: the whole request, as the trim budget counts it. It uses a fixed 2.5 chars/token, measured across 12,069 captures against real counts. Compare it to `usage.input_tokens` (the real total prompt) rather than treating it as exact. The trim budget deliberately uses a conservative 1.5 chars/token, so it never packs a prompt past the *context window*. This readout used 1.5 until 2026-08-07, which ran about 1.7x high. **A trigger condition on `estimated_total_tokens` written before that date wants re-scaling by 5/3**: a `{ $gt: 150000 }` threshold now silently stops firing where it used to.

Each section carries `name`, two sizes, and `role` (`system` / `prior_message` / `user`). It also carries an optional `group` label and an optional `content` body. The body is omitted when the `capture_context` preference is off, and truncated head and tail at 8,000 chars when it is on. The engine reads the preference before each model call, so a turn resumed after a question card follows a switch flipped while it waited.

**The two sizes answer different questions; picking the wrong one is the classic error.**

- `budget_delta_chars` is what the section ADDS to the request beyond what other sections count. Sum this one. The LLM Context Viewer divides the headline total by that sum, so the rows always add up to the number at the top. The headline is the measured `usage.input_tokens` when there is one, else the estimate.
- `content_chars` is the section's own size, measured whether or not the body was persisted. Ask it how big a region was. Never sum it.

On almost every section the two are equal. `Conversation` is the exception: every other section is already in the first message, so its delta is only what the tool loop added. Summing its real size would count the bundle twice.

Both are character counts. No section carries a token count.

**Pre-rename rows spell the delta `char_count`** and carry no `content_chars`. This is a renamed payload key, not a retired event name, so no `Legacy alias:` note applies. `GET /api/v1/events/:event_id/context` renames the key on the way out, but a direct SQL query over history sees it: read `coalesce(x->>'budget_delta_chars', x->>'char_count')`.

`trimmed` means the LLM got less than the assembled context, in **either** way. The trimmer's removal pass evicts whole messages, and its stubbing passes replace a body with a note. The note opens `[cut to fit the context budget:` and states the original size. Where the body had an event address, it also names the `events(action="query", event_id=...)` call that reads it back. Older rows reported only evictions, so stubbed turns showed as untrimmed.

`trim_passes` lists which passes did it, ascending, and is absent when nothing was trimmed. Pass 5 is the one that matters: it alone removes a whole message, while every other pass leaves an addressed stub the model can read back. So `trimmed: true` without pass 5 lost nothing silently. Rows from before the field carry no `trim_passes`, which means unknown, not none.

**Snapshot endpoint strips the heavy fields.** `GET /api/v1/threads/:thread_id/events` removes `sections` and `tools` from `ContextCaptured` and stamps `sections_stripped: true`. One capture can be ~50 kB, and a long session carries hundreds. Live SSE emissions still carry the full arrays.

To fetch them on demand (as the step-detail modal does), call `GET /api/v1/events/:event_id/context`, which returns `{ sections, tools }`. It is keyed on `event_id` only (UUIDs are unguessable, and scope matches the snapshot endpoint). Bulk consumers such as `exportThread.ts` pass `?include_context=true` on the snapshot instead of N+1 fetches. Triggers and subscribers rarely need `sections` / `tools`; if you do, use an on-demand path.

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

On the live SSE stream a `ToolResult` carries its full text, and a `CodingAgentToolResult` arrives already stripped. See `CodingAgentToolResult` in [coding-agent-events](coding-agent-events.md) for why.

**A tool call's `args` are stripped everywhere a client reads.** The snapshot and the live SSE stream both remove `args` from every `ToolCalled` and `CodingAgentToolCalled` and stamp `args_stripped: true`. Otherwise a `Write` would send a whole file to every client. `description` is filled first, so the step label never needs the args. A `generate_image` call keeps its args, because the image takes its alt text from the prompt.

To fetch the dropped args, call `GET /api/v1/events/:event_id/tool-args`. It returns `{ args }` for that one call. The stored row keeps them, so a trigger condition on `args.command` still matches, and `query_events` still returns them.

### `ChangeProposed`

```json
{
  "type": "ChangeProposed",
  "data": {
    "change_id": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
    "description": "Add ThreadEvent reference doc",
    "files": ["system-knowhow/thread-events.md"],
    "branch_name": "lucidos-claude-code-repo-lucidos-add-threadevent-reference-doc",
    "repo_root": "/Users/me/workspaces/dev"
  }
}
```

Every field except `change_id` is omitted at its zero value: `false`, `null`, an empty string or an empty list. So `requires_restart`, `hardened`, `incomplete`, `set_aside` and `origin` appear only when set. Read them defensively, as `payload.incomplete ?? false`. A live emit has no `commit_sha` key: the engine always sets it to `None`. Only legacy per-commit rows carry one.

Several events with the same `change_id` can arrive for a branch, one per proposing turn. The projection in `core::changes_projection` folds them into a single `changes` row.

### `ProposalWithheld`

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
    "change_id": "chg-2025-05-13-…",
    "requires_restart": false,
    "client_update": false,
    "commits": ["docs: add thread-events reference"],
    "thread_title": "Document all ThreadEvents",
    "actor": { "kind": "device", "device_id": "device-abc123" },
    "pre_merge_sha": "9b38db1b4…",
    "post_merge_sha": "a1b2c3d4e…",
    "path": ""
  }
}
```

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

**The parent is re-opened BY this callback, so it never has to wait for one.** The fan-in persists it on the parent and re-opens that thread with the status, summary and `pending_change_ids` an *event wait* would deliver. So an `await_event` (or `lucidos await-event`) on your own child's completion is redundant. The engine stands the callback down when a live wait covers it, so it is one turn either way. But the wait still spends recent-subscription budget, and its timeout can fire while the child works.

Await a `ChildThreadCompleted` only for a completion that is not your own child's, named with a `child_thread_id` condition. Matching is workspace-wide, so it can be any thread's child. The card lands on whichever thread is the parent, and the wait resolves off that row.

**One callback per completed turn, not one per child.** A *child follow-up* revives or redirects the child, and that turn's terminal sends a second `ChildThreadCompleted` for the same `child_thread_id`, on the same parent. A human clicking Continue on a coding-agent child does the same. So `child_thread_id` is not a key; the events are a log of completed turns.

These are **not completions**, and fire no card:

- **A steer.** A `ResponseCanceled` with cause `superseded_by_followup` is the mid-turn redirect when a follow-up lands on a live Codex turn. The child runs the redirected turn at once, and that turn's terminal is the report.
- **A terminal the engine is about to resume.** A coding-agent turn that dies on a transient upstream `API Error` emits a real `ResponseFailed`, and the engine resumes the session seconds later. That `ResponseFailed` and its `CodingAgentIdled` fire no parent callback, so the parent sees ONE card for the episode. The suppression depends on the resume being scheduled, never on the error. Past `MAX_API_ERROR_AUTO_RESUMES` the thread parks for good and the `failure` card fires (ADR 0199).
- **A user Stop.** A Stop on the child's running turn, or Cancel on its question card, ends the turn but not the child. The parent gets a `ChildThreadStopped` note instead, and no turn runs. The child's next finished turn sends this card as usual.
- **A turn that ends holding an event wait.** The wait wakes the child for another turn, and that turn reports.

If the user archives, deletes, or discards the child's change, this card arrives with status `canceled`. Until then the child counts toward the user's attention. A cancel you issue on your own child is not the user's Stop, and still arrives as a `canceled` card (ADR 0252).

Stop waiting and an agent standing its wait down wake nothing. So when an idle child's last wait ends that way, the card it held back arrives then. Archiving a waiting child sends a `canceled` card. A failed turn reports at once, wait or not (ADR 0254).

**Running more than one child at a time: `system-knowhow/orchestrating-sub-threads.md`.** This edge and the *child follow-up* are the only two carrying traffic between threads; nothing carries it sideways. That file is the manual for a parent coordinating several children. It covers what a child may do about a sibling's events, and how a ruling reaches a child that already finished.

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

- The user continues the child. Its next finished turn sends a `ChildThreadCompleted` with the real status.
- The user archives or deletes the child, or discards its change. A `ChildThreadCompleted` with status `canceled` arrives.
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

Reads still accept the legacy `task_id` / `task_name` aliases; new emissions use `trigger_id` / `trigger_name`.

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

- `tier: 0`: a finished worktree (clean, branch at main HEAD, no pending change) removed. The background worker waits out a short grace window first. The Disk Usage page's recommended cleanup removes it at once, on the user's request.
- `tier: 1`: build artifacts (`target/`, `node_modules/`, `.lucidos/cache/`) stripped from an idle worktree, which stays on disk. This happens, for example, an hour after its change was applied.
- `tier: 2`: the entire worktree directory removed. That covers the 30-day idle sweep and *stranded* worktrees. A stranded worktree's git admin dir under `.git/worktrees/<name>` is gone, so git cannot act on it.

`branch_deleted: true` appears only on a full removal that also dropped a fully merged branch. A stranded removal touches no refs, so there `branch_deleted` is always false.

### `ContinuationStarted`

```json
{
  "type": "ContinuationStarted",
  "data": {
    "branch": "lucidos-claude-code-repo-lucidos-…",
    "origin": { "kind": "engine", "reason": { "kind": "continuation_started" } },
    "reason": "auto_recovery_after_hang"
  }
}
```

`origin.reason.kind` is `continuation_started` (legacy alias `session_recovered`).

`reason` (optional) mirrors the originating `ContinuationRequested.reason`, so
the timeline names which interruption the resume recovered from:

- `user_clicked_continue`: a genuine resume after an engine restart (the user
  tapped "continue").
- `harden_requested`: the user pressed **Harden** on the Not ready strip, and
  the thread resumed to run `/harden`. Its turn end proposes the change.
- `auto_recovery_after_hang`: a hung subprocess OR a stray signal-kill where
  **nothing restarted**, e.g. another workspace's `cargo check` build-lock kill
  hitting this agent's process. The UI labels it "Resumed after the session
  stopped responding".
- `auto_resume_after_api_error`: the engine resumed after a transient upstream
  failure the agent reported itself. Labelled "Resumed after the model
  connection dropped".

Neither auto reason claims "Resumed after engine restart". The two are worded
apart because both can fire on one thread minutes apart. `reason` is absent on
legacy rows and the chat-rerun path.

**The boundary confers no thread type.** `ContinuationStarted` is emitted on all
three channels. `chat` and `trigger` come from `emit_resume_anchor` (reached from
`POST /api/v1/threads/<id>/continue` and the chat answer-resume path), and
`claude_code` from the coding agent's `--resume` dispatch. A thread is a
*coding-agent thread* because a `SessionStarted` opened an agent session on it,
never because it was continued. So the `thread_summaries` projection sets
`is_coding_agent` (and rewrites `source`) for every `SessionStarted`, but for
`ContinuationStarted` only on the `claude_code` channel. It never *clears* the
flag; repairing rows corrupted by an earlier write is a migration's job.

**Only an interrupted turn gets this boundary.** A thread parked on an
unanswered `UserQuestionAsked` is *preserved* across a restart (no
`ResponseAborted`). So answering it emits **no** `ContinuationStarted`: the
resumed work carries the original turn's `request_event_id` and continues that
exchange. A subscription on `ContinuationStarted` therefore fires for a revived
interruption, never for an answered question. "Parked" holds only while the
question is the newest thing on the thread.

If anything in `ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES` landed after it,
the card is dead. The thread then recovers as an ordinary interrupted turn, with
its boundary and its Continue button. See `coding-agent-events.md`
§ "An engine restart alone does NOT orphan a pending question".

## How a workspace would actually trigger on these

Any persisted `ThreadEvent` outside the blocklist (the four per-token streaming variants and the four side-question events) takes an `on` subscription directly:

```yaml
on:
  - event_type: ChangeApplied
    condition:
      hardened: true
run:
  intent: "Tell me which change just landed and what files it touched."
```

Four knobs:

1. **Pick the right event.** Lifecycle and one-per-turn variants are usually what you want. Always pair a per-action variant with a `condition:` filter (§ "Volume classes").
2. **Per-token streaming is off-limits**, and the create is refused. For token-level reactivity, consume the SSE stream directly.
3. **For workspace-defined signals**, `lucidos events emit` (or the `emit_event` LLM tool) writes a `SystemEvent::DomainEvent` that always reaches the matcher. Use it for a name outside the engine's ThreadEvent enum (e.g. `OuraDataImported`, `BuildBroken`); see `system-knowhow/lucidos-cli.md`.
4. **For workspace-scoped engine facts**, subscribe to the persisted `SystemEvent` directly. `BackupFailed`, `NotificationCreated` and `TriggerCompleted` are `on:` entries like any other. Never emit a domain event beside one to make it reachable: the engine already wrote the row.

Trigger-run failures auto-create an error notification, so "tell me when one of my triggers blew up" needs no wiring.

## Recipe-shaped guidance

Trigger config syntax (cron format, the `on` list, the per-entry `condition` operators): `system-knowhow/triggers.md`. Conditions are pure payload filters: each key is a field path into the event payload (the `data: { … }` object above).

The coding-agent slice (the `UserQuestion` vs permission distinction, the exact `CodingAgentIdled` field semantics, the no-`CodingAgentErrored` gap): `system-knowhow/coding-agent-events.md`.

Event-store column shape (`event_type`, `payload`, `created`, `aggregate`, `aggregate_id`, `sequence`) and the queries that walk threads from events: `.claude/rules/db.md`.
