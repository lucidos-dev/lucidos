---
name: ThreadEvent Reference
description: Every `ThreadEvent` the engine emits, by family (chat, coding-agent, lifecycle, changes, background bash, plugin, repo, merge conflict, transient SSE-only). With payload, persistence, and whether a trigger can subscribe. Load for "what events fire on a thread" or "can I trigger on ResponseGenerated / ChangeApplied / BackgroundBashCompleted". Also for "is X persisted" and "is X a real event name".
---

# ThreadEvent Reference

The complete enumerated list of `ThreadEvent`, the per-thread event family that flows through the EventBus into PostgreSQL, the SSE stream, and (for one curated entry) the trigger matcher. Source of truth: `crates/lucidos-engine/src/engine/thread_events/` (the enum itself is in `event.rs`). Variant names below are the **current** names; legacy aliases (e.g. `ClaudeCodeIdled`, `SessionRecovered`, `parent_thread`, `task_id` / `task_name`) exist as `#[serde(alias = ...)]` on the wire so old DB rows decode cleanly. Write new code, new triggers, and new docs against the current name only.

For the coding-agent slice (`CodingAgent*` + the `UserQuestion*` / permission machinery) the deep-dive lives in `system-knowhow/coding-agent-events.md`. This file is the master enumeration; the coding-agent entries below summarize and link.

For event-store column shape, the chat-mode terminator set, and the `events` table schema, see `.claude/rules/db.md`. For trigger config syntax (cron, the `on` subscription list, per-entry `condition` operators), see `system-knowhow/triggers.md`.

## One table, two enums

**This is the canonical statement of the `ThreadEvent` / `SystemEvent` split. Everything else points here; do not restate it elsewhere.**

There are two Rust enums and **one** table.

- **`ThreadEvent`** (`crates/lucidos-engine/src/engine/thread_events/`) is the per-thread family enumerated in this file: `MessageReceived`, `ResponseGenerated`, `CodingAgentIdled`, `ChildThreadCompleted`, every `Change*`, every `Thread*` lifecycle event.
- **`SystemEvent`** (`crates/lucidos-engine/src/engine/event_bus_system_event.rs`) is the workspace-scoped family: `NotificationCreated`, `TriggerCompleted`, `PluginInstalled`, `PreferencesChanged`, and so on. One of its variants, **`SystemEvent::DomainEvent`**, is the carrier for names the workspace itself invents (`HabitCompleted`, `OuraDataImported`, `LucidosReleased`), written by `lucidos events emit`, the `events` LLM tool's `emit` action, `lucidos.events.emit` in an app, or `POST /api/v1/events/emit`.

Both go through the one `EventBus::emit`. Every variant whose `is_persisted()` is true is written by the one `EventBus::persist` INSERT into the one **`events`** table, with `event_type` set to the variant name. A `DomainEvent` is stored under its *inner* type (`HabitCompleted`), never the literal string `"DomainEvent"`.

What actually differs is two columns:

| | `aggregate` | `aggregate_id` | `thread_id` column |
|---|---|---|---|
| `ThreadEvent` | `'thread'` | the thread id | set (mirrored from `aggregate_id`) |
| `SystemEvent::DomainEvent` | `'domain'` | the event type | NULL |
| other `SystemEvent` | its own (`'notification'`, `'trigger'`, `'plugin'`, `'app'`, …) | the entity id | NULL |

The rest of both enums is **transient**: a `ThreadEvent` whose `is_persisted()` is false (see "Persisted vs transient" below), or a `DomainEvent` emitted with `{ transient: true }`. Those are broadcast on SSE and never written at all, so they cannot be queried afterwards and cannot fire a trigger.

**Consequence for reading.** `GET /api/v1/events/query`, `GET /api/v1/events/count`, `GET /api/v1/events/types`, `lucidos events query`, `lucidos.events.query` in an app, and the `events` LLM tool all issue one statement over that one table with **no aggregate predicate**. They return persisted rows of BOTH enums, filtered only by `event_type` and time. There is no domain-event stream separate from a thread-event stream, and nothing to "switch to" in order to reach the other kind.

The worked case, because it is the one people get wrong: **`ChildThreadCompleted` is queryable by an app today.** It is a `ThreadEvent`, persisted, emitted on the **parent** thread by EventBus fan-in, so the row's `thread_id` is the parent and the payload carries `child_thread_id` / `child_thread_title` / `status` / `summary`. `lucidos.events.query({ event_type: 'ChildThreadCompleted' })` returns it with no engine change.

What the split IS, then: a Rust type distinction and a column value. It is never a storage boundary.

## Today the scheduler uses a blocklist

The scheduler subscribes to the EventBus and forwards events to the trigger matcher. It has two branches, one per enum, and they are gated differently. The `BusEvent::Thread` branch is gated by a small **blocklist** (`ThreadEvent::is_per_token_streaming` in `crates/lucidos-engine/src/engine/thread_events/event_impl.rs`; the gate itself lives in `crates/lucidos-engine/src/scheduler/mod.rs`). Every other persisted `ThreadEvent` is forwarded to the matcher and can be subscribed to via an `on:` entry on a trigger.

The blocklist contains exactly the per-token streaming variants — many fires per turn (one event per text chunk), never appropriate to subscribe a trigger to:

- `TextStreamed`
- `ThoughtStreamed`
- `CodingAgentTextStreamed`
- `CodingAgentThoughtStreamed`

The `BusEvent::System` branch is an **allowlist** instead, and its rule is one line: **a persisted `SystemEvent` is subscribable, a transient one is not** (ADR 0113). It admits every variant whose `is_persisted()` is true, plus a `DomainEvent` on either setting of `transient`. So `BackupCompleted`, `BackupFailed`, `NotificationCreated`, `TriggerCompleted`, `PluginInstalled` and the rest of the persisted set are all valid `on:` entries. `BackupProgress`, `Toast`, `MemoryRebuildProgress`, `RecoveryProgress` and `EmbeddingModelStatusChanged` are transient frames: they write no row, so they reach neither matcher. The full persisted list is `SystemEvent::PERSISTED_TYPE_NAMES`, and `.claude/rules/db.md` § Key event types enumerates the variants.

Both fan-outs run the one predicate, `core::event_subscription::is_subscribable_system_event`, so a trigger and an `await_event` are offered exactly the same set.

Per-action variants with high cardinality (`ToolCalled`, `ToolResult`, `CodingAgentToolCalled`, `CodingAgentToolResult`, `ContextCaptured`, `MemoryRecalled`, `ImageDescribed`, `UserPromptInjected`, `CodingAgentPromptSent`) are **triggerable**: they fire once per discrete action, scoped with per-entry `condition:` filters (e.g. `name: "Bash"`, `estimated_total_tokens: { $gt: 150000 }`). A condition key is a **field path**, so `args.command` reads one level down. Operators are `$eq` / `$ne` / `$lt` / `$lte` / `$gt` / `$gte` / `$in` / `$nin` / `$regex`, and a bare value means `$eq`. `$or` in key position takes a list of conditions. So a filter on the command text inside `args` is `{ "args.command": { "$regex": "cargo test" } }`.

**`thread_id` is available on every event in this file, and on none of their payloads.** The engine supplies it at matching time from the event's own thread, so `condition: { thread_id: "<uuid>" }` scopes a subscription to one thread whatever the event type: one coding-agent session reaching a turn boundary (`CodingAgentIdled`), one thread's next response (`ResponseGenerated`), one thread's next tool call. It is a matching-time field only. It never appears in a payload you read back with `query_events`, where the event row's own thread column carries it instead. A **`SystemEvent`** belongs to no thread, so a domain event and a frame like `BackupCompleted` both have no `thread_id` to filter on.

That means right now (each example below is one entry inside a trigger's `on` list, see `system-knowhow/triggers.md` for the full subscription shape):

- `event_type: UserQuestionAsked`: works. The typical use is "push me when an interactive question is raised so I can answer from my phone." Pair `send_notification` with `tap: { kind: 'navigate', to: { target: 'thread', id: '<thread_id>', event_id: '<source_event_id>' } }` so the tap deep-links straight to the question, see `triggers.md` for the worked example.
- `event_type: CodingAgentPermissionRequest` / `CommandPermissionRequested` / `McpPermissionRequested` / `CredentialRequested` — **work**. These are the other blocking-request events that should wake the user. `CommandPermissionRequested` is the chat command-guard card (ADR 0002); `McpPermissionRequested` is the chat MCP-tool card.
- `event_type: ResponseGenerated` / `ResponseFailed` / `CodingAgentIdled` / `ChangeApplied` / `ChangeHardened` / `TriggerCompleted` / `BackgroundBashCompleted` / every `Change*` / every `Thread*` lifecycle event — **work**.
- `event_type: ToolCalled` / `CodingAgentToolCalled` / `ContextCaptured` / `ImageDescribed` etc. — **work**. Use a per-entry `condition:` filter to scope; without one a chatty per-action variant will fire the trigger many times per turn.
- `event_type: TextStreamed` / `ThoughtStreamed` / `CodingAgentTextStreamed` / `CodingAgentThoughtStreamed`: **refused**, at both surfaces. The matcher never sees a per-token variant, and they would saturate whatever they were wired to.
- `event_type: <a workspace-emitted DomainEvent>` — works. `SystemEvent::DomainEvent` (emitted via `lucidos events emit` / the `emit_event` LLM tool) flows through the matcher unconditionally. This is the supported path for "trigger on something my workspace observes."
- `event_type: BackupCompleted` / `BackupFailed` / `NotificationCreated` / `TriggerCompleted` / `PluginInstalled` / any other persisted `SystemEvent`: **work**. There is no `thread_id` to condition on, so scope these with the event's own payload fields.
- `event_type: BackupProgress` / `Toast` / `MemoryRebuildProgress` / `RecoveryProgress` / `EmbeddingModelStatusChanged`: **refused**, at both surfaces. These are transient frames the engine broadcasts to the UI without writing a row. The refusal names the persisted event that ends the run. For a backup that is `BackupCompleted` or `BackupFailed`.

**Two more gates apply to every carrier.** First, self-exclusion. Every event a trigger's fire emits carries that trigger's id, and the matcher drops that one trigger from the matches. So a trigger is never woken by its own fire, at any depth. Every other subscriber still sees the event.

Second, the recursion cap, which bounds a chain running ACROSS triggers. An event a fire emits dispatches at that run's chain depth, one deeper than the event that fired it. Past `max_event_trigger_depth` (a *capacity policy* field, default 5) the event is still stored and still reaches SSE, it just fires no further triggers. The cap never touches an ordinary turn, whose events dispatch at depth 0.

**The two gates part company at a spawn.** Work a fire hands off emits UNMARKED, so a sub-thread or a coding-agent session it starts still wakes the trigger. That is what makes "wait for the session I started" work. The depth does reach that work. A sub-thread (`run_thread`), a coding agent (`run_coding_agent`) and a script all run at the fire's OWN depth, not one deeper. A hop is a trigger fire, counted once, so a spawn does not buy a fresh chain.

**When the cap stops a fire, you are notified.** It names the trigger, the event and the ceiling, once per trigger per ten minutes. A chain you meant to have would otherwise stop running with nothing to see. Raise `max_event_trigger_depth` in the Thread Queue panel if a chain is legitimately longer. See `system-knowhow/triggers.md` for what the two gates mean when you author a subscription.

If you add a new per-token streaming variant to `ThreadEvent`, add it to `ThreadEvent::is_per_token_streaming` in the same change. Lifecycle / one-per-turn / per-action variants need no scheduler change — they flow through by default.
A new `SystemEvent` variant needs no scheduler change either, because deciding it is persisted is what makes it subscribable.

The `Triggerable` column on every table below is the binary "would a trigger fire on this today?" answer. **Triggerable does not mean "good idea to subscribe without a condition"** — for any per-action variant, lean on `condition:` filters to scope the matches.

### Triggerable is not the same question as awaitable

An *event subscription* comes in **two species**, and they share a predicate language, a matcher (`EventSubscription::matches`) and a blocklist, but not their answer for every event:

- A **trigger subscription** is one entry in a *trigger*'s `on:` list, a persistent reactive rule. Each match spawns a NEW thread and leaves the subscription armed for the next one. "React to every X."
- A **thread subscription**, whose internal name is *event wait* (`await_event`), belongs to an existing thread. The calling thread finishes its turn and idles, the first match re-opens THAT thread with the event as a new message, and the subscription is spent. "Continue when the next X happens."

Two questions pick between them, and the first one is the one that gets forgotten:

1. **Where does the answer go?** A trigger reaches the user as a notification from its own thread; it cannot continue the conversation they are typing in. `await_event` re-opens the subscribing thread with a new turn, so the report lands in the thread they are reading. "Tell me **here** when X happens" is `await_event`, even though the phrasing sounds like a standing rule.
2. **How long must it last?** `await_event` is one-shot and you re-arm per event, with consecutive subscriptions capped. A reaction that must outlive the conversation and fire indefinitely is a trigger.

Being blocked is not a precondition. `await_event` is a delivery mechanism as much as a waiting one: a turn that could have ended perfectly well still uses it when the user wants the next X reported into this conversation. And `await_event` is not a stream: if you need every X forever, that is a trigger.

The two columns differ for exactly one family, the `EventWait*` events below. They are **triggerable but not awaitable**: a trigger that notifies "a thread's wait timed out" is a reasonable thing to want, while a wait on `EventWaitStarted` would satisfy itself the instant any thread in the workspace registers one. The per-token streaming blocklist applies to both.

Both species now validate their names the same way, so this is no longer a difference between them. See the next section.

## Check the name before you subscribe

`EventSubscription::matches` compares event types as **exact strings**. So a typo, a hallucinated name and a name a past release retired all read alike: the subscription arms, waits, and never matches. That silence was the whole failure class, and both surfaces now refuse it up front.

Every `on:` entry of a trigger, and every entry in `await_event`, is checked at subscription time. Three verdicts:

| The name is… | What happens |
|---|---|
| an engine name, misspelled or retired | **refused**, with the near match named (`CredentialStored` → "Did you mean `CredentialCreated`?") |
| outside the engine's set | **accepted**, with a warning when this workspace has never emitted it |
| a transient frame that really exists | **refused**, pointed at the persisted event that ends the run |

The middle row is the one that matters for your own work. A domain event you are about to start emitting is legitimate: "make X emit an event, then trigger on it" is the ordinary order. Nothing blocks it. The warning is there so a typo in your own name is still catchable.

**Look the name up rather than guessing.** The `events` tool's `event_types` action answers it in one call:

- **`engine`** is the closed set the validator checks against. A name read off this list always validates. A name that merely resembles one is refused.
- **`workspace`** is what this workspace's store has seen and the engine does not emit: your own domain events.
- **`retired`** is what the renames took away. **This is the complete list**, read straight from `ThreadEvent::LEGACY_TYPE_NAME_ALIASES`, which a test holds to the names serde still accepts. The `Legacy alias:` notes in the tables below are prose for a human reader and cover only some of it. Never hand-assemble the retired set by reading them: ask the tool.

For "has this ever actually happened, and how often", use the `count` action instead. With no `event_type` it returns the full per-type breakdown, so one call tells a name nobody has emitted from one that fired twice last year.

**The rename trap.** Old rows keep the old name, because an event is immutable once written. So `query_events({ event_type: "ClaudeCodeIdled" })` still returns history, and the name looks alive. Nothing emits it again. A subscription on it can only ever match the past, which is why a retired name is refused outright rather than warned about. Two live renames each took a live subscription with them: `ClaudeCodeIdled` to `CodingAgentIdled`, and `MemorySearched` to `MemoryRecalled`.

### The path is checked too, not just the name

A `condition` is the other half of a subscription, and it fails the same silent way. `{"version": "0.1.0"}` on `PluginInstalled` is syntactically perfect and matches nothing: that value sits at `manifest.manifest.version`. So every field path in a condition is checked against the twenty most recent stored payloads of that type. A path in none of them gets a warning, and the warning names the same leaf found deeper down.

It is a warning, never a refusal, because the sample is evidence rather than a schema. An optional field is legitimately absent from twenty rows, and `{"conclusion": {"$ne": "success"}}` deliberately matches an event carrying no `conclusion`. An event type with no stored rows says nothing at all.

**A condition names the unwrapped payload.** A persisted system event is stored as `{type, data}` and the matcher strips that envelope, so a condition writes `filename`, never `data.filename`. The `events` tool's `query` action gives you the stored row instead, envelope and all. Read a path there, then drop the leading `payload.data.` before writing it into a condition.

## Persisted vs transient

The enum splits into two halves, mirrored by `ThreadEvent::is_persisted()`:

All variants are past tense — events-only model, no command concept (imperative actions are reframed as request events like `AppUiRefreshRequested`). Persistence is orthogonal to tense:

- **Persisted.** `MessageReceived`, `ResponseGenerated`, `CodingAgentIdled`, `ChangeApplied`, etc. Written to the `events` table; replayable; visible to projections, history queries, and (in principle) the trigger matcher.
- **Transient.** `CumulativeTextUpdated`, `LlmCallRetried`, `AppUiRefreshRequested`, `PluginInstallRequested`, etc. Broadcast over SSE only. Never persisted; never reach the projection or trigger paths. Used for live UI updates (token streaming preview, modal-trigger request events) and child thread broadcasts.

A trigger on a transient event can never fire — the scheduler's matcher only looks at persisted events.

## Wire format and metadata

Persisted events are stored with `event_type` set to the variant name and `payload` as the variant's JSON object. Cross-cutting fields merged into the payload at persist time by `EventMeta` (see `crates/lucidos-engine/src/engine/thread_events/meta.rs` `EventMeta::apply`):

- `request_event_id` — links response/terminal events back to the originating request.
- `channel` — `"chat"` / `"claude_code"` / `"trigger"` (`EventChannel`). The `"claude_code"` wire string is the coding-agent channel for both Claude Code and Codex; the name is retained for compatibility with existing rows and clients.
- `actor` — `MessageOrigin` of who initiated. Stamped by mutating HTTP handlers via `api/actor::user_actor_resolved`.

Some variants (`ChangeApplied`, `ChangeDiscarded`, `ChangeReverted`, `ChangeApplyFailed`, `ChangeHardened`, `ThreadStarted`, `ThreadDiscarded`, `ImageUploaded`) carry `actor` as a per-variant field (predates `EventMeta`); `MessageReceived` and several others use `origin: Option<MessageOrigin>`. Treat both as the canonical "who did this" field for that event.

## Volume classes

Used in every table below. Pick the right class before subscribing a trigger.

- **lifecycle** — fires once at a moment in a thread's life (creation, archive, terminal). Safe to trigger on directly.
- **one-per-turn** — fires at most once per chat / coding-agent turn. Safe to trigger on, with a `condition` if needed.
- **per-action** — fires once per discrete user/agent action (one tool call, one message, one captured context). Triggerable, but always pair with a `condition` filter — without one, a chatty per-action variant will fire the trigger many times per turn.
- **high-volume-streaming** — many fires per turn (per token chunk). **Blocked by the scheduler** (`TextStreamed`, `ThoughtStreamed`, `CodingAgentTextStreamed`, `CodingAgentThoughtStreamed`); the matcher never sees them. Subscribing is a no-op.
- **transient-SSE-only** — never persisted; cannot trigger.

## Chat / agentic loop

These fire on chat threads (`channel = chat`) and on trigger-driven runs (`channel = trigger`). The coding-agent equivalents under "Coding agent" use the parallel `CodingAgent*` names.

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `MessageReceived` | A user (or upstream workspace, or parent thread, or engine) submitted text into the thread. Stamped at the HTTP boundary in `api/chat.rs::chat_submit`. Its projection also **clears the thread's stored compose draft** (the send is what ends that draft's life), advances the thread's *compose epoch* so a draft write composed before the send can no longer be applied after it, and broadcasts a `ThreadComposeChanged` reporting the emptied state and the new epoch, so peers mirroring the draft drop it live instead of at their next reload. Broadcast on every send to a thread that already existed, including one where no draft was stored: the epoch moved either way, and the draft-less case is exactly the one where a client's write is still in flight. A message that CREATES the thread announces nothing, since there was no compose slot to consume and no device has heard of the thread yet. | one-per-turn | yes | yes |
| `QueuedMessageRemoved` | A user removed a queued chat follow-up before the agentic loop ingested it. Pure append-only marker over the original `MessageReceived`: renderers hide the matching message only while it remains stepless, and the agentic loop skips the matching injected prompt when it drains the queue. Carries `removed_message_id: Uuid` (the event id of the queued `MessageReceived`), plus `actor` / `channel` from `EventMeta`. | per-action | yes | yes |
| `TextStreamed` | A complete chunk of assistant text was committed to the thread (post-stream finalize for chat; one event per appended chunk). | high-volume-streaming | yes | **no (blocked)** |
| `ThoughtStreamed` | The model emitted a `thinking` / reasoning block (extended-thinking models). One event per chunk. Legacy alias: `Thinking`. | high-volume-streaming | yes | **no (blocked)** |
| `ContextCaptured` | One snapshot of the LLM context the engine assembled for a single LLM call (prompt sections, tools, estimated tokens, real `usage` when the provider reports it). One per LLM call; the modal reads these to show context drift. **`purpose` says which call it was.** Absent means `turn`, an agent's own round trip, and that is every row written before the field existed. The other values are *auxiliary model calls*, ones the engine makes for itself: `title`, `image_describe`, `memory` (fact extraction and query classification), `conversation_summary` (the paragraph standing in for a thread's older assistant turns), `image_gen`, `voice` (one spoken reply from a *voice session*'s talker). One purpose per auxiliary model preference is a standing invariant (ADR 0107). A new one arrives with its own `model_*` / `reasoning_*` pair rather than sharing another's. An auxiliary row carries `producer: "auxiliary"`, no `tools`, a `context_window` of 0 (a single-shot call spends against no budget), and one body-less section sized to the request. It is recorded per ATTEMPT, so a resampled title and a retried extraction each leave more than one row: both spent tokens. The transcript never renders one. **A trigger on `ContextCaptured` now sees these too**, so scope it with `condition: { purpose: "turn" }` if you meant only agent turns. A `reconstructed: true` row was rebuilt after the fact by the startup backfill from events that were never captured live: its `estimated_total_tokens` is a reconstruction and it carries no `usage`. A cost rollup filtering on a present `usage` block therefore still reports measured spend only. **`usage.modality` splits the four counts across text, audio and image**, and only a realtime voice call reports one. Each part is named after the total it sums into, and a producer that cannot fill every field sends none. | per-action | yes | yes (use condition) |
| `MemoryRecalled` | The engine's **automatic pre-turn recall**: before the model saw anything, a classifier derived sub-queries, the chat-side memory consumer vector-searched long-term memory, and the hits were injected into the turn's context. Carries `results: usize` (how many were injected) and `queries: Vec<String>` (the classifier's sub-queries). **Not the agent's own lookup**: when the injection misses, the agent calls the `memory` tool's `search` action mid-turn with a query of its own, and that arrives as a `ToolCalled`. Subscribe to this one for "the engine recalled something", to `ToolCalled` for "the agent went looking". Legacy alias: `MemorySearched` (renamed 2026-08-12, because the two names were one word apart and read as the same event). Rows persisted under the old name still deserialize and still render, but **subscriptions do not follow a rename**: `on_event: MemorySearched` on a trigger, and an `await_event` waiting on that name, match the event type as an exact string and so stop firing. Re-point them at `MemoryRecalled`. | per-action | yes | yes (use condition) |
| `ToolCalled` | The chat agentic loop invoked a tool (`name`, `args`, optional `description`). Distinct from `CodingAgentToolCalled` — those are coding-agent tool calls. | per-action | yes | yes (use condition) |
| `ToolResult` | The result returned to the chat agentic loop for a prior `ToolCalled`. Carries `result: String`, `images`, `success: bool` (default true), and optional `tool_called_event_id: Uuid` set only by the post-restart recovery sweep (`recover_orphan_tool_calls`) for synthetic backfills: the frontend's `groupIntoExchanges` uses it to land the synthetic result in the same exchange as its orphan `ToolCalled` so the "Executing …" spinner resolves. Live emits omit it; chronological name pairing handles those. **Inline image bytes are stubbed out of `result`**, so a tool that returned an image persists something like `[image image/png, 641.2 KB omitted, not embedded in event]` or `[screenshot image/png, 1.5 MB omitted, not embedded in event]` followed by the page DOM. The model that made the call saw the actual image; the stub exists only because a megabyte of base64 per row made heavy threads unloadable. Reading one back via `query_events` means the image was shown and not persisted, never that it failed or was withheld. | per-action | yes | yes (use condition) |
| `BackgroundBashStarted` | A long-running task was spawned via `run_bash_background` (shell command) OR `run_python_background` (venv-rooted Python script — the engine wraps it as `bash -o pipefail -c "<venv-python> <script>"` and routes it through the same registry). The `command` field captures the exact shell invocation. Paired with a later `BackgroundBashCompleted`. | per-action | yes | yes |
| `BackgroundBashCompleted` | The task ended: natural exit, signal death, watchdog timeout, `bash_kill`, or the engine going away under it. Carries `exit_code: Option<i32>` (set **only** for a normal exit), `signal: Option<i32>` (set only for a signal death; omitted otherwise), `stdout`, `stderr`, `timed_out: bool`, `killed: bool`, `abandoned: bool`. Both `exit_code` and `signal` null means the status was unavailable. Never read that as success. `abandoned: true` is the engine-stop case, written by the teardown emit or the boot sweep, and it is NOT `killed` (which means `bash_kill`). Every started task on a live thread reaches exactly one of these, so a subscription on it is not left waiting on an event nobody will send. The audit-trail counterpart of `Started`. Emitting it does NOT evict the in-memory registry entry: a completed task stays drainable for a few minutes so a `bash_output` landing at the completion instant still gets the final tail, and `bash_output` falls back to this row only once that window closes. Same shape whether the spawning tool was `run_bash_background` or `run_python_background`. | per-action | yes | yes |
| `ResponseGenerated` | The chat agentic loop terminated with an assistant response. The chat-mode terminator. Carries `text` (`#[serde(skip_serializing_if = "is_empty_str")]`), `images`, `model`, `reasoning_effort`. **`text` may be empty**: when a turn ends on a clean, model-decided stop with no text and no tool calls (a *benign empty completion* — e.g. Gemini `finishReason: STOP` after successful tool calls), the loop emits an empty `ResponseGenerated` rather than `ResponseFailed`, so the thread completes Idle instead of showing a red error. The UI renders a neutral "model returned an empty response" note for an empty-bodied completion. See `classify_empty_completion` (`agentic_loop/helpers.rs`) for the benign-vs-failure split. | one-per-turn | yes | yes |
| `ResponseCanceled` | User clicked Cancel, clicked Apply / Discard / Archive on a still-running session, or posted a follow-up that interrupted a mid-turn Codex turn. Carries `cause: CancelCause` (`UserStop` / `UserAction` / `SupersededByFollowup` / `Unknown`). Always emit via `thread_events::emit_response_canceled` — it's idempotent against pre-emitted terminators (the `/api/v1/restart` race). | one-per-turn | yes | yes |
| `ResponseAborted` | System-driven termination — engine shutdown, safety net (non-watchdog), recovery sweep, OS signal, stale-projection settle. Carries `cause: AbortCause` (`EngineShutdown` / `SafetyNet` / `RecoveryAfterRestart` / `ProcessKilled` / `StaleSettle` / `SessionDropped` / `Unknown`). Always emit via `thread_events::emit_response_aborted`. Note: when a hung-subprocess watchdog interrupts a coding agent (vs a crash or driver death), the engine emits `ContinuationRequested{auto_recovery_after_hang}` instead of `ResponseAborted{SafetyNet}` so the thread auto-resumes without user intervention. Two watchdogs can fire that path — see the `ContinuationRequested` row below. | one-per-turn | yes | yes |
| `ResponseFailed` | Hard failure mid-turn: upstream API error, panic, OOM-killed bash, empty assistant text on a non-cancel turn (`agent_session::lifecycle::classify_result` triggers this for coding-agent threads too). Carries `error: String`. For an empty chat completion, `ResponseFailed` is reserved for the *genuine* failure shapes — output **truncated** (`max_tokens` / `MAX_TOKENS` / `length`), **blocked** by a safety/policy classifier (`refusal` / `SAFETY` / `content_filter`), **dropped output** (provider billed tokens but nothing parsed; Anthropic-only signal), or an **unrecognised** stop reason (fail-safe). A clean model-decided empty stop is benign and emits an empty `ResponseGenerated` instead (see that row). Classification is uniform across providers and thread types — see `classify_empty_completion` / `normalize_finish_reason` (`agentic_loop/helpers.rs`). | one-per-turn | yes | yes |
| `UserPromptInjected` | A user interjection (a message sent while the turn was already running) OR an engine-injected mid-flight message (resume note, child-thread callback in legacy paths) was relayed into the live agentic loop. Carries `text`, `mode: ActorMode`, optional `origin`, optional `injected_message_id`, optional `delivered_event_id`. The loop wraps the text before the model sees it (`framed_injected_prompt`, `agentic_loop/helpers.rs`), keyed on `mode` **and** on whether the message lands mid-turn or starts a turn of its own. Mid-turn: a `Human` message is framed as an interjection to answer *and then resume the work in progress* (a redirect still overrides, but answering alone is not a reason to end the turn), while `Agent`/`Engine` messages are framed as a system update to fold into the response in progress. An injection the previous turn ended before draining is re-processed as its own turn (`api::chat::process_orphan_chain`) and gets no resume directive at all: that turn is over, so there is no work left to carry on with. **Every** orphan in such a batch is announced, the first included (`announce_orphan_batch`). The re-processed turn reuses the already-persisted `MessageReceived` as its starter event, so this is the only event in it whose lifecycle rule sets the thread back to `running`, and the client uses `injected_message_id` to absorb it into that message's own panel rather than rendering a second one. The event itself always stores the user's raw `text`, never the framing. | per-action | yes | yes (use condition) |
| `ImageDescribed` | A background Flash call produced a text description for one of the images attached to a `MessageReceived`. One event per attached `user_image_hashes` entry, all carrying the same description text. Emitted from the agentic loop after iteration 1 of a chat turn; a message that arrives while the thread is already working is injected mid-turn instead of starting one, so that path emits from the chat injection fast-path as a detached task (before, such an image got no `ImageDescribed` at all and left no record once its bytes aged out of context). Replaced an in-place `jsonb_set` mutation that used to write `image_description` back into the source row. Carries `source_event_id: Uuid` (the originating `MessageReceived`), `hash: String` (the described blob's sha256), `description: String` (post `is_bad_image_description` filter), `model: String` (literal `"backfill"` on rows produced by the startup backfill, otherwise the actual Flash model). The `description` is indexed into memory (it carries real shared content — screenshots, tickets, photos), so an image-only turn isn't a memory black hole. | per-action | yes | yes (use condition) |
| `ConversationSummarized` | An auxiliary model compressed this thread's older **assistant** turns into one paragraph, cached so a later turn reuses it instead of re-rolling it (ADR 0102). Emitted from `load_chat_history` during turn setup, only when the summariser actually succeeds. Carries `summary: String` (the paragraph, exactly as `[CONVERSATION HISTORY]` renders it), `covers_through_event_id: Uuid` (the newest assistant turn the paragraph accounts for), `covered_count: u32`, and `model: String`. A later turn re-summarises only when this event is missing, or when the assistant turns past `covers_through_event_id` exceed `HISTORY_SUMMARY_REFRESH_AFTER`; otherwise it reuses the cached paragraph and renders the uncovered turns compacted. User turns never reach the summariser at all, so this only ever stands in for assistant work. The event is the cache: there is no table, so it survives an engine restart by construction. Not indexed into memory (the turns it summarises are already indexed individually). | per-action | yes | yes (use condition) |
| `TodoListWritten` | The *Lucidos Agent* called the `todo_write` LLM tool, OR the engine's `todo_consumer` settled the still-open items when the thread stopped working the list. Replace-whole-list semantics: `items: Vec<TodoItem>` is the new complete *todo list*, fully superseding any prior `TodoListWritten` in the thread. An optional `notes: String` rides beside the items and is replaced with them (ADR 0085's *todo notes*): absent on every list written without any, and on every row written before the field existed, so it is omitted from the payload rather than sent as null. No tool schema offers the field, so it reaches the handler only where the agent writes one anyway. The engine's settle carries it through untouched, since re-emitting the list without it would erase what the agent wrote. Each `TodoItem` has `content: String` (imperative form, "Run tests"), `active_form: String` (present continuous, "Running tests"), `status: TodoStatus` (`pending` / `in_progress` / `completed` / `waiting` / `abandoned`, snake_case on the wire). LLM tool handler enforces ≤ 50 items, at most one `in_progress`, and rejects BOTH `waiting` and `abandoned` (engine-only); empty list is valid and means "cleared". The engine-side `todo_consumer` subscribes to every terminator (`ResponseGenerated` / `ResponseCanceled` / `ResponseAborted` / `ResponseFailed`) and re-emits the latest list with every still-open item settled, so the panel shows an honest state once a response ends. **Which status it settles to answers "parked or walked away?":** `waiting` when the thread still holds a live *event wait* as of that terminator (`await_event` does not hold the turn, per ADR 0049, so a subscribed thread terminates normally and sleeps), `abandoned` otherwise. `waiting` is itself still open, so a wait that resolves without the agent picking the list back up settles to `abandoned`; `abandoned` is terminal and a later subscription never reverses it. The consumer's **second trigger is `EventWaitCanceled`**, and it exists because a terminator is not the only moment a thread stops being parked: `EventWaitDelivered` and `EventWaitExpired` each write a `UserPromptInjected` re-entry anchor, so the re-entered turn's own terminator settles the list, while a cancel re-opens nothing and would otherwise leave an idle thread reading `waiting` with no next terminator to correct it. It is skipped when a turn still owns the list (thread status `running` / `waiting_for_user_answer` / `paused`, which is the agent standing its own watch down mid-turn), since `abandoned` is terminal and that turn's terminator could not walk it back. Cancelling N subscriptions in one cascade still writes at most one settle: at cancel k the waits above it are unresolved at that sequence, so the target is `waiting` again and the already-settled short-circuit holds. Chat-agent tool only: *coding-agent threads* render backend-native todo/tool output instead. Under *self-curated context mode* there is no `todo_write` at all, and the same event is emitted from the `[TODO]` heading of the agent's *working understanding*. UI: frontend walks the thread's events backwards, finds the most recent `TodoListWritten`, and renders the items in the prompt-bar collapsible panel; abandoned rows render with a dashed strike-through and an `abandoned` tag, waiting rows with a clock marker, full-strength text and a `waiting` tag. | per-action | yes | yes (use condition) |
| `WorkingUnderstandingWritten` | The *Lucidos Agent* wrote its *working understanding*, as a marked span of ordinary text inside its own reply. Carries `document: String`, the whole of what the thread now holds, which fully supersedes any prior row on the thread. Two forms reach it: a replace sets the document, and an add appends to its body and its constraints. Only the body and the constraints are stored. The checklist goes to `TodoListWritten`, and the held-open addresses are applied and dropped, so a rewrite cannot re-assert a keep it made ten rounds ago. An entry may carry the `evt-<hex>` address of the event it is about, which reads the original bytes back through `events(action="query", event_id=…)`. Nothing in the body is parsed, so a missing address costs a read, not a write. Only a line under `[KEEP OPEN]` must be an exact address, and one that is not is refused with a fault. Thread-scoped and NOT long-term memory: what the thread learned about the world is the extractor's job. Emitted only where the *self-curated context mode* preference is on, and only from chat and trigger threads: a *coding-agent thread* builds its own context. Rendered in the transcript as a folded step, never as chat prose. | per-action | yes | yes |

Terminator set for chat-mode (`TERMINATOR_EVENT_TYPES` constant): `ResponseGenerated`, `ResponseCanceled`, `ResponseAborted`, `ResponseFailed`. Used by `has_terminator_for` for idempotent terminator emission.

**One turn, one terminator, even during a restart.** An interruption produces two would-be terminators: the engine pre-emits `ResponseAborted` for the turn it is tearing down, and the agentic loop's cancel arm fires moments later when its token is cancelled. They collapse into one because both name the same turn through `request_event_id`, so `has_terminator_for` sees the abort and `emit_response_canceled` skips. That only holds because the pre-emit resolves the anchor from the running turn itself (`engine::in_flight_request_event_id`) rather than guessing it from the newest `MessageReceived`. Guessing is what produced the stacked "Paused by restart" plus "Response canceled" pair on a thread nobody cancelled: a follow-up queued mid-turn, or a turn started by `ContinuationStarted`, made the two ids disagree and both boundaries rendered. Applies to all three out-of-loop emitters: the `/api/v1/restart` teardown, the 60 s stuck-turn eviction, and the shutdown sweep.

**A coding-agent session gets there a different way, because the request id cannot do that job for it.** A chat turn takes a fresh anchor per turn, but a live coding-agent session keeps ONE `request_event_id` for its whole life, across every follow-up turn, so "a terminator already exists for this request id" would read the first turn's `ResponseGenerated` as covering the second and swallow a real terminator. Its suppression is therefore scoped to the *turn*: a `ResponseAborted` that out-sequences the thread's newest start event (`MessageReceived`, `CodingAgentUserMessageSent`, `TriggerStarted`, `ContinuationStarted`, `OrphanRecoveryStarted`) already covers the turn, so the session emits nothing. The in-memory `AgentSession::external_terminal_emitted` flag is the fast path for a boundary emitted while the session was already registered; the events-table check covers the opposite ordering, a boundary emitted **before the session existed at all**. Both restart emitters iterate a snapshot of `agent_sessions`, so a session still spawning is invisible to them and has no flag to be handed. Without that second arm, a *Switch to new version* landing inside a multi-second session spawn stacked a `ResponseCanceled{user_stop}` next to "Paused by restart" on a turn nobody stopped, and because a cancel means "turn ended" to the resume gate, the next boot then declined the auto-resume and withdrew the promise the transcript had already made.

## Resume / continuation

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `ContinuationStarted` | Resume-after-abort boundary. Opens a new exchange in the timeline whose body is the rerun (chat: re-LLM call after abort; coding agent: resume into the same `cc_session_id` when the backend has one). **Channel-agnostic** — emitted on the chat, trigger, and coding-agent paths alike, so it says nothing about a thread's type (see below). Carries optional `branch`, engine-stamped `origin`, and `reason` (mirrors the `ContinuationRequested.reason` so a hang/stray-signal auto-recovery isn't labeled an engine restart). Aliases for old DB rows: `SessionRecovered`, `SessionResumed`. | lifecycle | yes | yes |
| `SessionStarted` | A coding-agent process spawned. Carries `session_id` (backend session id), optional `branch`, optional `repo_id`, plus *coding-agent-thread* discriminators: `coding_agent_kind` (`"lucidos" \| "app" \| "external"`, default `"lucidos"`), `coding_agent_folder` (canonical folder the spawn targets: `<ws>/data/apps/<id>/` for App, repo root otherwise), `app_id` (set only for App), and `coding_agent` (`"claude-code" \| "codex"`, default `"claude-code"`, which backend drives the thread; locked in by the first SessionStarted via the `thread_summaries.coding_agent` projection). Legacy rows without these decode as Lucidos / Claude Code via the serde defaults. Its projection also **clears the thread's stored compose draft** and advances the *compose epoch*, because a coding-agent session start consumes the thread's prompt, and **broadcasts a `ThreadComposeChanged`** carrying the emptied state and the new epoch. Both are gated on the event being a coding-agent one: a chat or trigger `ContinuationStarted` shares this arm, consumes nothing, and must leave the user's draft and the epoch alone. | lifecycle | yes | yes |
| `SessionEnded` | A coding-agent thread is truly done (terminal-only). Carries `reason: SessionEndReason` (`Shutdown` / `Panic` / `Closed` / `StaleResume` / `LegacyNonTerminal`). `StaleResume` is the one transient case: it does NOT settle the thread — the projection stays `running` while the caller re-spawns once with a fresh session (chat and the continuation/spawn consumer both do), and the frontend skips the AbortPanel. A caller that does not retry leaves the thread `running` with no subprocess, so the retry is part of the contract, not an optimization. | lifecycle | yes | yes |

## Coding agent (Claude Code / Codex)

The umbrella `CodingAgent*` family covers Claude Code and Codex (the variants carry `coding_agent: CodingAgent`, default `ClaudeCode`; the wire field has `#[serde(alias = "agent")]` so legacy DB rows persisted before the rename still decode). Each variant has a `#[serde(alias = "ClaudeCode<X>")]` for the legacy pre-rename variant name — write new code against the `CodingAgent*` form.

**See `system-knowhow/coding-agent-events.md` for full payload shapes, the `CodingAgentIdled` field-by-field reference, the `UserQuestion` vs `CodingAgentPermission` distinction, and the no-`CodingAgentErrored` gap.** This table is the index.

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `CodingAgentUserMessageSent` | A user message was relayed into the agent's input stream. | one-per-turn | yes | yes |
| `CodingAgentPromptSent` | An engine-synthesized prompt was injected (orphan-recovery, hardening retrigger, merge-conflict explainer, post-question continuation). Carries `origin: Option<MessageOrigin>`. Audit-only, not rendered in chat. | per-action | yes | yes (use condition) |
| `CodingAgentTextStreamed` | One chunk of the coding agent's assistant text. | high-volume-streaming | yes | **no (blocked)** |
| `CodingAgentThoughtStreamed` | One chunk of the coding agent's streamed reasoning/thinking (CC's `thinking_delta`; Codex's `item/reasoning/*Delta` or `reasoning` item). Coalesced before persistence. Rendered as the live "Thinking" step's content. | high-volume-streaming | yes | **no (blocked)** |
| `CodingAgentToolCalled` | One coding-agent tool invocation. Carries `name`, `args`, optional `description`, `tool_use_id`. | per-action | yes | yes (use condition) |
| `CodingAgentToolResult` | The result returned to the coding agent for a prior `CodingAgentToolCalled`. Same `tool_use_id`. | per-action | yes | yes (use condition) |
| `CodingAgentIdled` | **The coding-agent turn-boundary marker.** Emitted at the end of every coding-agent turn whose Result wasn't an engine-shutdown abort. Carries `has_changes`, `is_external_repo`, `requires_restart`, `cc_session_id`, `coding_agent`, optional `reason`, optional `worktree_path`, optional `worktree_head_sha`, `bg_bash_pending` (recorded-history flag: true when the turn idled with a chat-agent `run_bash_background` task still running; **no longer gates proposal or drives any UI** — the change proposes at idle regardless of background bash, and correctness is covered by harden-at-apply). | one-per-turn | yes | yes |
| `CodingAgentSettingsChanged` | User changed model, reasoning effort, or permission mode mid-session — and also emitted once at backend init carrying `cc_session_id` (and `claude_config_dir`, the `CLAUDE_CONFIG_DIR` the session was created under) when available so both are durable before the first `CodingAgentIdled`. The session id lets a mid-turn engine restart still resume; the config dir lets the resume re-inject the right `CLAUDE_CONFIG_DIR` so CC finds the transcript even if the user toggled the env var mid-flight. | lifecycle (rare) | yes | yes |
| `CodingAgentPermissionRequest` | A coding agent asked to confirm a tool call, on one of two raise paths. Claude Code's MCP permission-prompt subprocess fires for a path outside the session's working directories, or `.git/` inside the worktree. Those directories are the worktree, the workspace's `data/` tree and `/tmp`; an in-worktree write, including under `.claude/`, is auto-allowed before any card. Under the `auto` permission mode CC's classifier decides instead. The Codex app-server approval bridge fires for a sandbox-escaping `command_execution` or an out-of-worktree `file_change` under `approvalPolicy: on-request`. The exec escape-hatch protocol emits none. | per-action | yes | yes |
| `CodingAgentPermissionResolved` | The above request was answered, or auto-resolved by the engine: recovery (orphaned after restart), supersession (`allowed: false`, `reason: "Superseded by a new message"` when the user types instead of clicking), or a **session-ended clear** (`reason: "Coding agent session ended before answering — request expired"` when the turn idled with the card still dangling — e.g. a workflow whose parallel subagent's card outlived the main turn). Carries `allowed`, optional `reason`, optional `persist_scope` (`narrow` / `broad` / `session`). Flips the thread back to `running` **only from `waiting_for_user_answer`** — a resolution on an already-idle/terminal thread (a stale click hours after idle, or the session-ended clear) leaves the status unchanged, so it can't zombie a finished thread into a dead `running`. | per-action | yes | yes |
| `MissingHardeningDetected` | Engine detected a coding-agent session ended without running `/harden` and auto-spawned a recovery hardening session. Not a session terminator — the thread stays active until hardening finishes. | lifecycle (rare) | yes | yes |
| `ContinuationRequested` | Continuation marker, emitted when an interrupted coding-agent turn needs to resume without a new user message. Picked up by the spawn dispatcher; the event id is the spawn idempotency key. Delivery is guaranteed two ways: the dispatcher opens its bus subscription before its startup backfill runs (so a request emitted during startup is buffered, never lost), and at every engine start it re-dispatches any still-unactuated request (one with no later lifecycle or terminal event on a thread still `running`), so an emitted request can't silently strand the thread as a running zombie. `reason: String` is one of: `"user_clicked_continue"` (user clicked Continue after an engine restart), `"answered_after_idle"` (user answered an `AskUserQuestion` after the coding-agent subprocess was torn down at idle), `"auto_recovery_after_hang"` (a hung-subprocess watchdog detected the coding agent silent past its inactivity limit and auto-resumes without user intervention), `"auto_resume_after_switch"` (recovery auto-resumes an in-flight coding-agent thread after a user-initiated *Switch to new version*), `"auto_resume_after_api_error"` (the coding agent ended a turn on a transient upstream failure it reported itself, an `API Error: …` such as a connection closed mid-response, and the engine resumes the session instead of leaving the thread dead behind the `ResponseFailed`). The API-error resume is the only BOUNDED reason: at most 3 in a row, counted since the thread's last `MessageReceived` or `ResponseGenerated`, so a persistently broken upstream surfaces as a red dot instead of looping. It is deliberately skipped during engine shutdown (post-restart recovery owns those threads) and on a conflict-resolution session (an API drop mid-merge still aborts the merge and leaves the change pending). Two watchdogs can produce `auto_recovery_after_hang`: the in-loop one inside `run_session`'s `select!` (10 min, fast first line) and an external scanner task (12 min, ticks every 30 s from outside any per-thread loop, which catches the case where the `select!` itself is wedged in an event-handler await). The two share a gate, the 2-min grace ensures the in-loop fires first when it can. The gate normally skips while a tool is in flight (legitimate silence: a long `Bash`, an unanswered `AskUserQuestion`), but that skip is bounded by a 45-min hung-tool ceiling: a tool that never returns (a hung sub-agent) past the ceiling fires anyway, after re-confirming the thread is still `running` so a pending user answer is never euthanized. When the thread has an in-flight *conflict resolution* (a pending change whose latest merge-lifecycle event is an unpaired `MergeConflictDetected`, see that row), a **recovery-shaped** continuation (`user_clicked_continue` / `auto_recovery_after_hang` / `auto_resume_after_switch`, never `answered_after_idle`, which is a different interaction and must not silently land a change) re-attaches the merge duty: the resumed session runs in the merge worktree with the change bound, so its completion finishes the apply (`ChangeApplied`) or aborts for real. A stray-killed merge session's cleanup deliberately skips the failure events so this pairing stays open for the hand-off instead of failing the apply the user is watching. A duty the continuation cannot carry (merge worktree gone, resume failed before settling it) is closed loudly with the deferred `MergeResolutionCleared` + `ChangeApplyFailed` pair. Past name `ContinueSignal` is kept as a serde alias for old DB rows. | lifecycle | yes | yes |

## Question / permission machinery

Not prefixed `CodingAgent*` because the same machinery serves any agent that needs to ask the user a structured question. All the blocking-request events below are triggerable: wire a trigger to any of them to push the user when the agent needs an answer. See `triggers.md` for the deep-link pattern (`tap: { kind: 'navigate', to: { target: 'thread', id, event_id } }`).

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `UserQuestionAsked` | An interactive question raised — by Claude Code's `AskUserQuestion` tool, Codex's `ask_user_question` MCP tool (one question per call), or the chat agent's `ask_user_question` tool. `meta.channel` (`claude_code` — the coding-agent channel, both backends — / `chat`) records which lane raised it. Resume happens via `POST /api/v1/threads/{thread_id}/answer-question`; the engine branches on the channel to fire the right resume side-effects (coding-agent: resume marker + `ContinuationRequested` respawn if needed; chat: wake the in-process tool). Carries `tool_use_id`, `cc_session_id` (empty for chat-channel and Codex rows), `question`, `options: Vec<QuestionOption>`, optional `worktree_path`, `multi_select: bool`. | one-per-turn (of the `Asked` kind) | yes | yes |
| `UserQuestionAnswered` | The user (or, on the orphan-recovery path, the engine) supplied an answer. Pairs 1:1 with the matching `UserQuestionAsked` via `tool_use_id`. Carries `answer: AnswerKind` (`Selected` / `FreeText` / `MultiSelected` / `Canceled` / `Superseded`) and propagates `meta.channel` from the originating `Asked`. The last two resolve the question without the user answering it: `Canceled` is a dismissal or teardown stamp, `Superseded` is a follow-up that could not be the answer and replaced the question instead (coding-agent lane only). | one-per-turn | yes | yes |
| `CommandPermissionRequested` | The **command guard** (ADR 0002) paused a chat bash/python tool call to ask the user — the `IrreversibleDanger` lane (a likely real-world side-effect — mutating HTTP, sending mail, a cloud-CLI mutation — or destruction outside the workspace). For the ambiguous middle the lane is decided by the LLM **judge** (Phase 3), after a static fast-path settles the obvious safe/catastrophic cases. The chat mirror of `CodingAgentPermissionRequest`: it renders the same `PermissionCard`, but the agent loop blocks in-process (no MCP subprocess). Carries `request_id`, `tool_use_id`, `tool_name` (a bash/python tool), `command` (the inspected text), `summary` (the card's one-line risk, written by the judge). Chat-channel only; flips the thread to `waiting_for_user_answer`. | per-action (only when the guard is on AND a command hits the danger lane) | yes | yes |
| `CommandPermissionResolved` | The above was answered (Allow once / Deny / Allow for this thread / Always allow), or auto-resolved by the engine (`reason: "Superseded by a new message"` when the user types instead of clicking; an orphan/cancel reason on restart or Stop). Carries `request_id`, `allowed`, optional `reason`, optional `persist_scope` (`narrow` / `broad` / `session`). Flips the thread back to `running` **only from `waiting_for_user_answer`** (a stale resolution on an idle/terminal thread leaves the status unchanged). | per-action | yes | yes |
| `McpPermissionRequested` | The Lucidos Agent (chat) paused an **MCP server tool** call to ask the user — the chat mirror of `CommandPermissionRequested` for MCP tools. Renders the same `PermissionCard`; the agent loop blocks in-process. Carries `request_id`, `tool_use_id`, `server_id` (MCP registry key), `server_name` (human label), `tool_name` (bare MCP tool), `arguments_summary`. Chat-channel only; flips the thread to `waiting_for_user_answer`. **Skipped (no event, auto-approved) in two cases**: a non-interactive **trigger** thread (no human to prompt) and a server with the `auto_approve` flag set. | per-action (only when the call isn't pre-authorized) | yes | yes |
| `McpPermissionResolved` | The above was answered (Allow once / Deny / Allow for this thread / Always allow this tool / Always allow this server), or auto-resolved by the engine (superseded / orphan / cancel). Carries `request_id`, `allowed`, optional `reason`, optional `persist_scope` (`narrow` → `Mcp(server:tool)`, `broad` → `Mcp(server:*)`, both persisted to the workspace's `mcp-allowed-tools`; `session` → in-memory per-thread). Flips the thread back to `running` **only from `waiting_for_user_answer`** (a stale resolution on an idle/terminal thread leaves the status unchanged). | per-action | yes | yes |
| `CommandCheckpointed` | The **command guard** (ADR 0002, Phase 4) bracketed a `ReversibleDanger` command (in-workspace deletion/overwrite) with two snapshots of the workspace's git-visible content: a **pre** image on a safety ref before it ran, and a **post** image after. Diffing the pair is what tells the engine which files the command created, overwrote and deleted, so the card can offer both an Undo and a view of what changed. Emitted **after** the command returns, and only when the two images differ: a command that changed nothing git-visible (typically because its target was gitignored) emits nothing, since its Undo could neither restore nor remove anything. A failed snapshot likewise emits nothing and lets the command run unguarded. Carries `checkpoint_id` (the ref key), `command` (the inspected text), `summary` (the card line), and the counts `restores` / `removes` (what Undo would put back, and what it would delete because the command created it; both 0 on events written before the counts existed). Does not change thread status. | per-action (only when the guard is on AND a command hits the reversible lane AND it changed something git-visible) | yes | yes |
| `CommandCheckpointReverted` | The user clicked Undo on a `CommandCheckpointed` card (or the engine resolved it): the workspace was restored from the pre image and the files the command created were removed, each only if it still matched what the command wrote. The two refs are kept, so the card's diff stays viewable afterwards. Carries `checkpoint_id`; stamped with the original turn's `request_event_id` so it groups into the same exchange as its checkpoint (the card renders reverted). | per-action | yes | yes |
| `CredentialRequested` | Persisted audit-log entry: a credential prompt was opened for `provider`. Pairs with the transient `CredentialPromptRequested` SSE request that carries the JSON payload for the modal. | lifecycle | yes | yes |
| `McpConsentRequested` | Legacy persisted audit-log entry (`tool`, `args`) from the pre-card MCP consent flow. No longer emitted — chat MCP consent now uses the in-thread `McpPermissionRequested` / `McpPermissionResolved` permission card above. Kept as a defined variant for replay of any historical rows. | lifecycle | yes | yes |

`QUESTION_OVERTAKEN_EVENT_TYPES` constant — the unified set of event names that mean a `UserQuestionAsked` is no longer the latest interactive point on the thread. Once any of these lands after a question, the next typed user text starts a fresh follow-up rather than a `FreeText` answer. Two categories: **terminal** (`ResponseAborted`, `ResponseCanceled`, `ResponseFailed`, `CodingAgentIdled`); **agent progression** — coding agent (`CodingAgentTextStreamed`, `CodingAgentToolCalled`, `CodingAgentToolResult`, `CodingAgentPromptSent`) and chat (`TextStreamed`, `ThoughtStreamed`, `ToolCalled`, `ToolResult`). The coding-agent progression category defends against the parallel-tool-call race: a coding agent can emit a question alongside sibling tool calls in one assistant message, the question path blocks while the siblings dispatch and emit events. Without filtering on those events, the user's next typed comment is silently absorbed as a `FreeText` answer to the dead question.

Because the fast-path reroutes typed text, an answer that carries composer text (`FreeText`, or `MultiSelected` with `text`) **clears the thread's stored compose draft** (but only when the draft is exactly what was submitted: trimmed text compare, and no attached images, since an answer carries none) and **broadcasts a `ThreadComposeChanged`** carrying the now-empty state and the thread's new *compose epoch*, so every device learns of it live and a draft write composed before the answer can no longer be applied after it. Nothing else on this path would do either: no `MessageReceived` is emitted, so the send-side clear never runs, and the draft would otherwise re-sync to every device (or linger on peers until their next thread-summary reload). A click-only answer submits no text, so it clears nothing and broadcasts nothing, and a different draft still in progress on another device survives.

The FreeText fast-path is additionally gated to **human-authored** follow-ups (`ActorMode::Human`). Only a real person typing answers an open question; agent- and engine-driven re-entries on the same thread are not the user's answer and must fall through. The case that motivated this: a **child-thread completion** re-opens the parent via `notify_parent_of_child_completion` with `ActorMode::Agent`, feeding a `[CHILD THREAD COMPLETED] …` block through the same chat-turn entry point. Before the guard, that block was consumed as a bogus `UserQuestionAnswered { FreeText }` (actor = `thread_link`/`child`), silently killing the user's open question. Now the re-entry falls through to the injection fast-path (queued as `ReentryFromEngine`), so the question stays live for the user and the child's result is processed right after they answer it.

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
| `TriggerStarted` | A scheduled or event-driven trigger run started. Carries `trigger_id`, optional `trigger_name`, optional `prompt`, optional `invocation: TriggerInvocation` (`Schedule` or `Event { event_type, event_id?, thread_id? }`, where `thread_id` is set only for thread-scoped source events and is exposed to script triggers as `TRIGGER_EVENT_THREAD_ID`), optional `origin`, `go_to_review: bool`, optional `model` and `reasoning_effort`. Aliases on the wire: `task_id`, `task_name` (legacy from when triggers were called "scheduled tasks"). `model` / `reasoning_effort` record what the fire actually ran on (the trigger's own pin, else the account chat default). This is a trigger thread's *starter* event and it has no `MessageReceived`, so those two fields are where the per-thread model memory reads from: a follow-up on a trigger thread reuses the fire's model instead of snapping back to the account default. Absent on runs recorded before the fields existed. | lifecycle | yes | yes |
| `TriggerCompleted` | A trigger run finished. Carries `trigger_id`, optional `trigger_name`, optional `result_summary`. Same aliases. The engine guarantees `result_summary` is a non-empty, single trimmed line for every run it emits — when a script exits 0 with no stdout it falls back to the script's last non-empty line, else `"<name> completed (exit <code>, no output)"`; an intent run with no final text falls back to `"<name> completed (no output)"`. So a blank summary never surfaces and idle-detector triggers don't read as a no-op fire flood to the learning/audit sweeps. | lifecycle | yes | yes |

## Changes (per-thread coding-agent change proposals)

The change family is per-thread — `change_id` is the primary identifier. `ChangeProposed` is emitted **once per coding-agent turn**, at end-of-turn, gated by `may_touch_change_state_at_idle` (only on `TerminalKind::Generated`; aborts/cancels/failures don't propose). That's the "coding agent is done with real finished work" contract: Apply/proposal state waits for idle. The Diff button is separate git truth and can become available earlier when the worktree post-commit hook refreshes `coding_agent_has_diff`.

`files` carries the branch's net diff, so the event is also how an **existing** change is corrected: when later commits cancel the diff out, the same `change_id` is re-emitted with `files: []`, which re-syncs the row to zero files, clears `requires_restart`, and — because `coding_agent_has_diff` is derived from `files` rather than hardcoded — stops claiming the thread has a diff. The change stays `pending`; only the user resolves it. An empty-`files` emit never *creates* a row (the reconcile refuses when the branch has no pending change), so it can't invent an empty change for a diffless session.

**A coding-agent thread holds at most one pending change at a time.** A thread works on one branch/worktree; when a merge-conflict re-run (or any re-spawn) proposes on a *new* branch, `propose_change` first discards the thread's pending change(s) on the *old* branch(es) — emitting `ChangeDiscarded` for each **before** the new `ChangeProposed` (so the discard's flag-clear can't wipe the freshly-set `coding_agent_proposed`). As a backstop, `apply_change` runs the same reconcile after a *successful* apply — gated on the change actually landing (`ApplyStatus::Applied`, never `Noop`/`Hardening`/`Conflict`, which would discard a newer sibling) — so it covers the panel Apply, the no-live `apply_now` paths, and the Apply-All driver in one place; the live in-place merge path reconciles in `apply_now_success`. Without this, an orphaned `pending` row lingers on the abandoned branch and the frontend (which treats *any* pending row for the thread as "has pending changes") keeps offering Apply/Discard and never Archive. Same-branch multi-change is preserved (reconcile keys on branch/`change_id`, not "everything else"), and `discard_change` notifies the Apply-All driver so discarding a sibling that is itself a batch member advances the batch instead of stalling it. See `docs/plans/2026-07-01-orphaned-pending-change-blocks-archive.md`.

Legacy: historical events with empty `change_id` + `commit_sha` set are from the old per-commit git hook (deleted along with `commit_hook.rs` + `/api/v1/internal/commit-made` to enforce the "never auto-propose for unfinished work" rule). The projection still handles them on replay, but they're inert — no `thread_summaries` updates, no row inserts into `changes`. The current post-commit hook emits no `ChangeProposed`; it only refreshes `coding_agent_has_diff` through the internal `coding-agent-diff-refresh` endpoint. The DB-level `changes` table is a projection over the aggregate events only.

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `ChangeProposed` | End-of-turn aggregate emit from `propose_change` (the coding agent finished a turn `Generated` with worktree changes). Carries `change_id` (non-empty UUID), optional `description`, `files`, `requires_restart`, optional `origin`, `commit_sha: None`, `branch_name`, `repo_root`, `hardened`, `incomplete`, plus legacy `path` / `diff` for old rows. `incomplete: true` only on engine-internal recovery paths (orphan recovery, stale-session cleanup) that surface commits whose originating turn was killed before closure; a subsequent clean-Generated turn re-emits with `incomplete: false` and clears the flag. Legacy per-commit shape (empty `change_id`, `commit_sha` set) exists only in historical events and is inert in the projection. | per-action | yes | yes |
| `ChangeApplied` | A change was merged to main. Carries `change_id`, `requires_restart`, `client_update`, `commits: Vec<String>` (subjects, oldest first), optional `thread_title`, optional `actor`, optional `pre_merge_sha` / `post_merge_sha` (used by Revert), legacy `path`. | lifecycle | yes | yes |
| `ChangeDiscarded` | A pending change was discarded. Carries `change_id`, optional `actor`, legacy `path`. | lifecycle | yes | yes |
| `ChangeReverted` | An applied change was reverted. Carries `change_id`, optional `actor`, legacy `path`. | lifecycle | yes | yes |
| `ChangeApplyFailed` | Apply attempt failed mid-merge. Carries `change_id`, `error`, optional `actor`. | lifecycle | yes | yes |
| `ChangeHardened` | The change's working tree was hardened (`/harden` marker stamped on HEAD). Idempotent — projection treats only the latest event per `change_id`. Implicitly downgraded when a fresh `ChangeProposed` arrives with `hardened: false`. | lifecycle | yes | yes |
| `MergeConflictDetected` | Engine detected a merge conflict pulling main into a coding-agent branch. Carries `change_id`, `files`, optional engine-stamped `origin`. Also the open half of the *conflict-resolution duty* pairing: while a pending change's latest merge-lifecycle event is a `MergeConflictDetected` (no closing `MergeResolutionCleared` / `ChangeApplyFailed` / `ChangeApplied` / `ChangeDiscarded` yet), a conflict resolution is in flight, and any recovery-shaped continuation of the thread re-attaches that duty so the resumed session finishes the apply (see `ContinuationRequested`). | lifecycle (rare) | yes | yes |
| `MergeResolutionStarted` | A merge-resolution worktree was set up. Carries `change_id`, `worktree_path`, `temp_branch`. Survives restart so startup cleanup can find dangling worktrees. | lifecycle (rare) | yes | yes |
| `MergeResolutionCleared` | The merge-resolution worktree was torn down (cleanup finished). Carries `change_id`. | lifecycle (rare) | yes | yes |

## Cross-thread / context

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `ChildThreadCompleted` | A child thread spawned by `run_thread` / `run_coding_agent` reached a terminal event (coding agent: `CodingAgentIdled` or `SessionEnded`; chat: `ResponseGenerated` / `ResponseFailed`). Emitted on the **parent** thread by EventBus fan-in, so the row's own `thread_id` is the PARENT. Fires once per completed TURN, so a child that was followed up on (or continued) reports again. Carries `child_thread_id`, optional `child_thread_title`, `status: ChildCompletionStatus` (snake_case on the wire: `success` / `failure` / `no_changes` / `canceled`), `summary` (truncated to 2000 chars; indexed by `indexable_text`), `pending_change_ids` (omitted from the payload when empty). Queryable by an app via `lucidos.events.query({ event_type: 'ChildThreadCompleted' })`, see § "One table, two enums". | per-action | yes | yes |
| `ContextDismissed` | **Retired by ADR 0109 and still readable.** Nothing emits it any more: `dismiss_from_context` is gone, because under *self-curated context mode* the *swept window* takes a result on its own. Existing workspaces hold rows, and the resume helper still honours every one of them, so a body an agent dropped before the change stays dropped. Carries `dismissed_event_id`, the *handle* of the event the body came from. | per-action | yes | yes |
| `ContextKeptOpen` | The agent set one tool result's clock back to zero, by writing its address under a `[KEEP OPEN]` heading in its *working understanding*. Carries `kept_open_event_id`, the *handle* of the `ToolCalled` behind the result. Same-thread only, and only that type: a keep moves the clock on a `tool_result` block, and nothing else is one. The keep is applied where the span is parsed, so this event is the durable record rather than the mechanism. It applies once, from the reply that wrote it. It exempts the item from no pass: the trimmer at the wall takes held items last and still takes them. Reaches only a workspace running *self-curated context mode*: everywhere else nothing is swept, so a keep would say nothing. | per-action | yes | yes |
| `WorktreeCleaned` | Background worktree cleanup ran on this thread (Phase 10.2/10.3). Carries `tier: u8` (0 = applied/clean worktree removed after the short grace; 1 = build artifacts stripped, worktree still on disk; 2 = entire worktree removed — the full-removal tier, also used for *stranded* worktrees whose git admin dir is gone), `freed_bytes: u64` (best-effort), `branch_deleted: bool` (a full removal that also dropped a fully-merged branch; always false for stranded removal). | lifecycle (rare per thread) | yes | yes |

## Event wait (a thread holding a subscription)

The lifecycle of one `await_event` call. All four are persisted, all four are
**triggerable but never awaitable** (see § "Triggerable is not the same question
as awaitable"). There is no `thread_event_waits` table: `EventWaitStarted` *is*
the wait, and the dispatcher's live set is rebuilt from these rows at boot.

| Event | When it fires | Volume | Persisted | Triggerable | Awaitable |
|---|---|---|---|---|---|
| `EventWaitStarted` | A thread registered a subscription. Emitted between the `ToolCalled` and that call's `ToolResult`, so the pair closes normally and the turn carries on. Carries `wait_id`, `tool_use_id`, `on: EventSubscription[]` (same shape as a trigger's `on:`), `reason` (the model's own words, shown to the user), `armed_at`, `expires_at`, and `watermark` (the event `sequence` at registration, which the catch-up scan reads forward from). `armed_at` is recorded rather than derived from `expires_at`, because the age is what `list_event_waits` reports and a derived one drifts; rows written before 2026-08-07 lack it and fall back to the event row's own `created`. Writes NO status. | per-action (rare) | yes | yes | **no** |
| `EventWaitDelivered` | A matching event resolved the wait. Carries `wait_id`, the matched `event_id` / `event_type` / `payload` (self-contained, so replay never dangles), and `matched_index` (which `on:` entry fired). | per-action (rare) | yes | yes | **no** |
| `EventWaitExpired` | The wait passed `expires_at`. **Re-opens the thread** with an explanatory message rather than dropping it: a silently dropped wait is a permanently stalled thread, which is worse than the polling this replaces. Carries `wait_id`. | per-action (rare) | yes | yes | **no** |
| `EventWaitCanceled` | The subscription was stopped short of its own resolution. `cause` is one of `user_stop` (the **Stop waiting** button), `agent_stand_down` (the agent retired one of its own), `thread_archived`, `thread_discarded`. Note what is absent: neither an ordinary user message nor a thread-level **Stop** disturbs a subscription in any way. `thread_canceled` is a RETIRED cause, still read so pre-2026-08-07 rows replay, never emitted. Also carries `on` and `reason`, a copy of what was stopped, so the transcript entry is self-contained on replay the way a delivery is: a stop renders at its own place in the timeline and its `EventWaitStarted` is routinely outside the loaded window by then. Both are absent on pre-2026-08-07 rows. One side effect the other two resolutions do not have: because a cancel re-opens nothing, it is the moment `todo_consumer` settles a *todo list* the thread parked, unless a turn still owns it (see `TodoListWritten`). | per-action (rare) | yes | yes | **no** |

### A subscription does not hold the turn

`await_event` returns immediately, like any other tool. The turn carries on and
ends with an ordinary terminator, and the thread is then plain **`idle`** while
it watches: no queue slot and no blocking state. Archive stays offered, and
archiving cancels the subscription rather than stranding it. What surfaces a
live subscription is the per-thread **waiting indicator**, not the thread status.

**One thing does wait with you: a change you already proposed.** The thread
reads as *Waiting* rather than *Changes to review*, and Apply and Discard are
withheld until nothing is left to wake it. You will resume on the delivery and
may commit again, so the change is not final yet. Applying it would merge a
branch you are still working on. The user's way out is **Stop waiting**, which
ends the subscription and returns both buttons. So a turn that parks with a
change pending is telling the user "not yet", not "here you go".

#### It already happened: the registration result may hand you the answer

A subscription watches **forward only**, so it can never fire for something that
has already gone by. If the thing might be in the past, check state first as you
would anyway; arming a wait for an event that already happened just idles until
the timeout.

What you do not have to worry about is the **race between that check and the
call**. If a match landed in the few minutes just before it, registration finds
it and names it in the `await_event` result, with its payload and how long ago
it was.

**That is a report, not a delivery.** The subscription watches FORWARD from the
moment it was armed, so it will never fire for anything the result names, and a
turn that ends without acting on it ends with the thing unhandled. It is not
delivered to you because only you can tell an event you missed from one you
handled yourself earlier in the same turn. Act on it now, or decide out loud
that you already did.

Nothing suppresses that report except an event you were literally handed by an
earlier wait (an `EventWaitDelivered`), so a re-arm right after a **delivery** is
never told about the event it was just handed.

Read that promise narrowly: it covers a delivery, not every re-entry. A
*child-completion* callback is the other way an event re-opens a thread, and the
fan-in writes no `EventWaitDelivered`, so if you re-arm on
`ChildThreadCompleted` in such a turn the report can name the very callback that
re-opened you. Recognise it by its `child_thread_id` and its age and carry on. Better
still, do not subscribe to your own child at all: see the `ChildThreadCompleted`
section below for why that wait buys nothing.

#### Which resolutions leave you still watching, and which do not

| Wake | Subscription after it | What to do |
|---|---|---|
| **Delivery** (`EventWaitDelivered`) | **Spent.** The first match resolves the wait and consumes it. Any *other* live wait on the thread is untouched. | This one has stopped watching. To catch the next one, call `await_event` again *before the turn ends*. Saying you will re-subscribe is not re-subscribing: a turn that ends with no new call leaves nothing watching for it. |
| **Expiry** (`EventWaitExpired`) | Gone. | Report what you were waiting for, rather than subscribing again to the same thing. |
| **Stopped** (`EventWaitCanceled`) | Gone, because somebody stopped it: the **Stop waiting** button, an archive or discard, or you standing it down yourself. There is no re-entry at all, so the thread is left exactly as it was. | Report back. Do not re-register unless they ask. |

Delivery is the one that bites. It is the only resolution that consumes the
subscription *and* hands you a payload to act on, so it reads like the wait is
still running when it is not. A standing in-thread watch is therefore one
subscription per event, and it is bounded by the consecutive-subscription cap in
§ "Limits" below: past it the next `await_event` call is refused and you have to
report back. Do not promise the user "forever" in a thread; that is a trigger's
job.

A **user message** is deliberately absent from that table, and its absence is
the point: it resolves nothing, so every subscription survives it with its
deadline intact and none of them needs re-registering. It used to *detach* a
wait, which was the closest thing to a fourth row.

A thread-level **Stop** is absent for the same reason, as of 2026-08-07. Stop
ends the running turn; it does not touch a subscription, which was never holding
that turn. It used to stop all of them, which meant pressing Stop on one turn
silently killed unrelated watches armed hours earlier.


**No `EventWait*` event writes a status**, and that absence is the rule rather
than an omission. Registration happens mid-turn, so the turn's own terminator
decides. A resolution lands on a thread that is either idle (its own
`UserPromptInjected` sets `running`) or running something unrelated, which a
write here would misreport as revived.

**A user message changes nothing.** Typing into a subscribed thread runs an
ordinary turn and every subscription survives untouched, deadline included,
because none of them was holding anything.

This was not always so. Until 2026-08-06 a wait was **attached**: `await_event`
ended the turn with its `tool_use` deliberately unpaired, so the delivered event
could arrive as that call's result and the model could resume mid-thought inside
one exchange. It bought continuity, and it cost an unpaired `tool_use` in the
message array, which is a provider 400 the moment anything else runs on the
thread. Paying for that needed detach-on-interruption with a filler result, an
attachment probe at every resolution site, a `was_attached` field on all three
resolutions, two anchor shapes, a `waiting_for_event` status, a restart
preserve guard, and a bar on the injection fast path. All of it is gone. See
`docs/plans/2026-08-06-every-event-wait-is-detached.md`.

### The re-entry anchor

Every delivery and every expiry is immediately followed by exactly one more
event, which is where the payload goes: a **`UserPromptInjected`** carrying it as
prose. It starts a new exchange, which is the honest shape for something that may
arrive hours later, and it is the same shape a child-thread completion uses to
re-open its parent.

**It says the event arrived, never that you were asleep.** Registration does not
hold your turn, so a match can land while this thread is still working, and the
engine then folds it into the running turn and tells you it arrived "while you
were working". The transcript card reads `Event arrived: <type>` for the same
reason: nothing about a delivery knows which of the two lanes it took.

On a *delivery* it also carries `delivered_event_id`, the id of the
`EventWaitDelivered` above it. The prose is the prompt the model reads and
cannot be trimmed, but a client rendering it verbatim shows a screen of
pretty-printed JSON, so the id points at the row already holding the same facts
as fields (`event_type`, `payload`) and the transcript names the event with its
payload folded away. An *expiry* leaves the field unset: it has no payload to
point at.

Worth knowing when reading a transcript, and load-bearing on restart: a
resolution followed *only* by its anchor is one whose turn never ran, which is
how the engine re-drives a re-entry lost to a crash.

### Both agents, one registration

The chat agent registers through the `await_event` LLM tool. A **coding agent**
registers through `lucidos await-event` (see `lucidos-cli`), which POSTs
`/api/v1/threads/<id>/event-waits` into the same code, so the caps, the
subscribability gate and the refusal wording are one implementation rather than
two. The delivery routes down the coding-agent lane (into a live session, or a
fresh resume when there is none), exactly as a child completion does.

Coding agents were excluded from waits in v1 for one reason: the engine does not
own a Claude Code or Codex session's message array, so it could not leave a
dangling `tool_use` in one. Removing the attached shape removed the obstacle.

### Limits

Three, all refused at the registration boundary with an error the agent reads in
the same turn rather than discovering later:

- `timeout_secs` is **required** and capped at **24 hours**. There is no
  unbounded wait. For anything longer, the right shape is a trigger.
- A thread may hold **25 live waits** at once, and may not register the same
  `on:` list twice (one event would then be delivered twice). The limit is on how
  many separate re-entries can be outstanding, not on how much you can watch: one
  wait's `on:` list is uncapped, so watching a dozen things in one subscription
  (any entry delivers) costs one of the 25.
- A thread may subscribe **10 times in a row** with no message from the user in
  between. That bounds a thread that re-opens itself, two threads ping-ponging,
  and a model simply stuck. An agent- or engine-authored message does not reset
  the count, since those are exactly what such a loop is made of.

## Voice session (a thread being spoken to)

The lifecycle of one *voice session*. Voice is a **mode of a chat thread**, never
a kind of thread (ADR 0148). So a session leaves the thread's `source` at `chat`,
opens no `channel` of its own, and moves neither status nor section: a live
microphone is not a turn.

Voice is **experimental and off by default**. None of these events can occur
until a workspace sets the `voice_enabled` preference.

There is no voice-session table. The two lifecycle rows below *are* the session,
which is what lets the boot sweep find one whose engine died mid-call.

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `VoiceSessionStarted` | A voice session opened on this thread. Carries `session_id: Uuid`, and the `actor` (from `EventMeta`) is the device that opened the socket. Exactly one session may be live per thread, so a second upgrade is refused and writes no second row. Placing a call bumps the thread's recency and nothing else. It does NOT promote a draft: connecting is not a conversation, so the first spoken word does that instead (ADR 0167). | lifecycle (rare per thread) | yes | yes |
| `VoiceSessionEnded` | The session closed. Carries the same `session_id`, `duration_secs: u64`, and `reason`: `hangup` (the caller rang off), `disconnected` (the socket died with no goodbye), `provider_failed` (the talker could not go on), `engine_shutdown` (the engine went away under the call, or the boot sweep settled a start its process never got to end). A sweep-settled row carries `duration_secs: 0`, because the engine holding the clock is gone. | lifecycle (rare per thread) | yes | yes |
| `SpokenReplyGenerated` | The talker finished saying something out loud, and this is what it said. Carries `session_id: Uuid`, `text: String`, and `interrupted: bool` (the caller spoke over it, so only that much was heard). One per talker turn, whether the talker composed the words itself or was reading the agent's answer aloud: both are what the caller heard. A reply cut off before a word was said writes nothing. The `actor` names the talker as a guest agent, so the agent reading the thread sees it under its own speaker label rather than as its own prior turn (ADR 0150). It is `Metadata`, because the agent's turn owns the thread's status and a talker turn landing mid-turn must not settle it. Like the spoken message beside it, it MAKES THE THREAD REAL (ADR 0167). That matters because the talker usually greets first. | a few per call | yes | yes |
| `SpokenMessageReceived` | The caller said something and the talker answered it alone, from what it already knew. Carries `session_id: Uuid` and `text: String`, and the `actor` is the caller's device rather than the talker. It started no agent turn, which is exactly why it is not a `MessageReceived`: that variant is a Start event, and using it here would leave the thread claiming a turn that never runs. `Metadata`, and it moves no section. It MAKES THE THREAD REAL, as the spoken reply beside it does (ADR 0167): a draft the call was placed from becomes an ordinary thread, its stored draft is cleared, and every device is told. The caller's FIRST spoken words also become the thread's `first_message`, which is what titles a call nobody delegated from. | a few per call | yes | yes |
| `WorkDelegated` | The talker called its one tool, asking for the agent. Carries `session_id: Uuid` and `reason: String`, the talker's own few words on what the caller wants. Never empty: a call with no reason gets a stand-in rather than being dropped. The `actor` names the talker as a guest agent, as on a spoken reply. It sits BESIDE the `MessageReceived` that started the turn, never in place of it, so it is `Metadata` and moves no section. | a few per call | yes | yes |

No payload here carries audio, and audio is never persisted at all.

**The talker decides whether a spoken turn needs the agent.** It holds exactly
one tool, `delegate`, taking a short reason. What it delegates persists as a
`MessageReceived` and starts the agent's turn through the same single-flight
admission a typed message uses. What it answers alone persists as a
`SpokenMessageReceived` and starts nothing.

Either way the utterance is recorded exactly once, so a call's transcript is
the thread's transcript: the caller's words, the agent's answer where there was
one, and the `SpokenReplyGenerated` rows for what was actually said out loud.

**The caller's words are always written before the reply to them.** Both rows
of a talker-only exchange leave one handler, so the order they are emitted in
is the order a reader meets them.

That `MessageReceived` carries `voice_session_id`, naming the session it was
spoken on. It is the only thing that tells the message apart from a typed one,
because the composer stays live during a call.

**The tool is an ask, not a wake.** The talker is never told whether an agent
turn is already running, and never has to be. It calls `delegate` for every
request needing the agent, including one made while earlier work is still
going. Single-flight admission then decides whether that starts a turn or
joins one.

**What the talker says is not what the doer wrote.** The written answer is
handed over to be SAID, never read: a spoken reply is what it means, in the
talker's own words. Past 400 characters the talker gives the headline and asks
whether the caller wants the detail. The full text reaches the talker either
way, so a "yes, go on" is answered from what it was given rather than invented.
The reader has that full text in the thread.

What a session SPENT is not here. It lands as one `ContextCaptured` per spoken
reply, carrying `purpose: "voice"`. So a cost rollup reads voice through the
surface it already reads every other model call through.

## Transient — never persisted, broadcast over SSE only

All transient names are past tense (events-only model). They cannot trigger (the matcher only sees persisted events). They drive live UI state (streaming preview, modal opens, in-app refreshes) and parent-thread fan-out signals. The "request events" carry the JSON payload that drives a frontend modal; the persisted sibling `CredentialRequested` is the audit-log entry that the same request opened a prompt. (The old `McpConsentPromptRequested` transient request was removed — chat MCP consent is now the persisted in-thread `McpPermissionRequested` card, not a modal.)

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
| `AppUiRefreshRequested` | Tells any open app iframe with `app_id` to reload itself. Legacy alias: `RefreshAppUI`. | per-action |
| `AppUiCaptureRequested` | Asks an open app iframe to capture state for `request_id`. The reply lands via the SDK capture path. Legacy alias: `CaptureAppUI`. | per-action |
| `NavigationRequested` | Tells the frontend to navigate (URL, intra-app route, etc.). Carries `payload: String`. An agent navigate (`navigate_ui`) also carries an optional `actor` (the originating device — the device that sent the prompt that triggered the turn); the frontend scopes the navigate to that device so it doesn't land on the user's other devices. Absent for trigger/background turns and the SDK app-iframe (nil-thread) path. | per-action |
| `CodingAgentThreadSpawned` | A child coding-agent thread (spawned via `run_coding_agent` / `run_thread`) has started. Carries `cc_thread_id`, `title`, `agent`. SSE-only — the persisted record of the child is its own thread row. Alias: `CcThreadSpawned`. | per-action |
| `CodingAgentDiffChanged` | A coding-agent worktree post-commit hook reconciled `coding_agent_has_diff` and the value changed. Carries `has_diff` and a full thread aggregate on SSE so the frontend can show or hide the Diff button immediately. Does **not** imply `ChangeProposed` / Apply readiness. | per-action |
| `ChildrenCountChanged` | A parent or ancestor thread's aggregate metadata changed. Carries the full updated aggregate (`active_children_count`, `total_children_count`, `blocking_descendant_count`, `attention_descendant_count`, …). Fires when (a) a direct child terminates and the parent's active/total counts shift, or (b) any descendant's "blocking" or "attention-needing" predicate flips (Running, WaitingForUserAnswer, or `has_pending_changes` && CodingAgent — see `is_blocking` / `is_attention_needing`), in which case every ancestor on the chain receives the broadcast with its updated counts. Drives the "Active children" badge, the cascading-archive button-hide (via `blocking_descendant_count`), and the Current-bubble routing in `display_section` (via `attention_descendant_count`). | per-action |

## Indexable text

`ThreadEvent::indexable_text()` returns `Some(&str)` for variants whose body should be indexed into the memory store: `MessageReceived`, `UserPromptInjected` (only without `injected_message_id`; with one it merely echoes a `MessageReceived` that is already persisted and already indexed, so indexing it too would file the same sentence twice), `ResponseGenerated`, `ResponseCanceled`, `ResponseAborted`, `ChildThreadCompleted`, and `ImageDescribed` (its `description`, the only textual record of an image-only turn). All others are `None`.

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
    "device": "My MacBook",
    "parent_thread_id": null,
    "spawning_event_id": null,
    "mode": "human",
    "model": "claude-opus-4-7",
    "reasoning_effort": null,
    "origin": {
      "kind": "device",
      "device_id": "device-abc123",
      "label": "My MacBook"
    }
  }
}
```

`mode` is `ActorMode` (`human` / `agent` / `engine`). `origin` is the structured `MessageOrigin` (`Device` / `Api` / `Workspace` / `ThreadLink` / `Engine` / `System`). Old DB rows may be missing `origin` — the frontend's `legacyOrigin()` synthesizes from `device_id` / `parent_thread_id`.

`voice_session_id` is present only when the message was **spoken** on a *voice session*, and it names that session. Absent means typed, which is every row written before voice existed. It has to live on the message. Voice is a mode of a thread, so the composer stays live during a call. A typed message therefore sits between the same pair of session events a spoken one does. The transcript reads it to mark the bubble as spoken.

The `Api` variant carries an optional `source_thread_id`:

```json
"origin": {
  "kind": "api",
  "user_agent": "curl/8.7.1",
  "mode": "agent",
  "source_thread_id": "9c1f-..."
}
```

Set when the engine recognised the request as coming from a Lucidos-spawned subprocess (coding-agent session, `run_bash`, `run_python`, scheduled script, `lucidos` CLI). Detection is via the **thread-bound origin token**. Every spawned subprocess gets its own `LUCIDOS_AGENT_ORIGIN_TOKEN` injected into its env, shaped `<thread-id>@<depth>@<trigger>.<mac>` under a per-engine-startup HMAC secret. A `-` stands in for any field the spawn does not carry. The `lucidos` CLI auto-forwards the token as the `x-lucidos-agent-origin-token` header on every engine call, and the Python shim does the same for urllib and requests.

The MAC covers the whole prefix, so all three fields are authenticated rather than claimed. `source_thread_id` is the first of them, and a subprocess can present only the token it was handed. The depth is the event-trigger chain depth (ADR 0138), and the trigger is the fire this subprocess is (ADR 0137). Mutating HTTP handlers (`apply_change`, `revert_change`, `discard_change`, `chat_submit`, settings writes, …) then stamp `Api { mode: "agent", source_thread_id: <spawning thread> }`, whatever the request body claims. So agent actions never appear as "You" cards.

A token that does not verify, including a valid one re-pointed at another thread, is treated as not-a-subprocess at all. It falls through to the regular unattributed-API-client resolution, the same path external API clients take. A script that shells out to bare `curl` sends no header at all, so its call takes that same path. There used to be a second `x-lucidos-source-thread-id` header carrying the thread id. It was unverifiable, so any subprocess could claim any thread. It is gone, and nothing reads one.

Cross-thread chat injection from a subprocess is refused with 403 at `chat_submit`. The target must be the caller's own thread, or a thread that does not exist yet and whose declared parent is the caller. See `api::chat::subprocess_chat_legitimate` for the full allow/deny matrix.

#### `mode` and `origin` are attribution, and an agent may not fabricate them

The two fields together are the answer to "who authored this turn", and the
projection acts on `mode`, not just the UI: `human` sets the thread's
`initiator` to the user and bumps `last_user_action`, the drawer's recency sort.
So a `mode: "human"` turn an agent wrote is not a cosmetic mislabel, it is a
record the user cannot distinguish from their own.

**An agent must never post a message the engine would record as human.** The
engine enforces this on both chat entry points: `mode: "human"` is accepted only
from a caller carrying a `device_id` that resolves in the `devices` table (the
user's own client, which sends `x-lucidos-device-id` on every mutating request)
or a `caller_workspace` (the cross-workspace contract, where the calling
workspace vouches for its own human). Everything else is an *unattributed
caller* and gets 403. See `api::chat::human_mode_is_attributed`.

Note the asymmetry this removes. Before it, a subprocess that PRESENTED its
origin token was held to `subprocess_chat_legitimate` (which refuses
`mode: Human` outright), while the same subprocess shelling out to `curl`
dropped the token, read as an ordinary external API client, and was allowed.
Dropping your credential bought more privilege than presenting it. It no longer
does.

Two related refusals on the same path, both of which write nothing:

- **404** when `thread_id` names no existing thread and the request carries no
  create signal (`new_thread: true`, a `parent_thread_id`, or a
  `caller_workspace`). A thread has no creation event, so an unknown id used to
  be materialized by this event's own upsert projection, which meant a caller
  that reached the wrong engine got its threads created there and its read-back
  confirmed the mistake.
- **409** when the request asserted a different workspace than the answering
  engine serves (`x-lucidos-target-workspace`). The body names the actual
  workspace.

If no tool covers what you were asked to do, say so. Do not hand-roll HTTP to
the engine to get around it: see `system-knowhow/lucidos-cli.md` § "Never post
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

`parent_thread_id` is the *callback linkage*: it is what makes the launching thread resume when this one finishes, what increments its `active_children_count`, and what the `thread_summaries` projection stores. The origin is *display attribution*: who to name and link in the message route popover. A `relation: "child"` spawn carries both. Two shapes carry the origin with **no** linkage, and both are deliberate:

- a *top-thread* (`relation: "top"`), which names its *spawning thread* but reports back to nobody;
- a *child follow-up*, which attributes the message to the parent without re-counting a child that already exists.

So do not infer parent-ness from a `ThreadLink` origin. Read `parent_thread_id` for that. `resolve_attend_mode` is the worked example: it walks the origin chain to decide whether a coding-agent permission card can be auto-resolved with the root trigger's side-effect grant, and hops only where the linkage is present, so a top spawn asks a human instead of inheriting a grant.

`image_description` on this payload is **deprecated** — it survives only on legacy rows persisted before the `ImageDescribed` past-tense event existed. New `MessageReceived` emissions always serialize it as `null` (the field is `Option<String>` with `skip_serializing_if = Option::is_none`). Read the description from `ImageDescribed` instead, joined by `source_event_id`. The startup backfill emits one `ImageDescribed` per legacy `(source, hash)` pair so the new event-based read path covers historical rows too.

### `QueuedMessageRemoved`

```json
{
  "type": "QueuedMessageRemoved",
  "data": {
    "removed_message_id": "550e8400-e29b-41d4-a716-446655440000",
    "channel": "chat",
    "actor": {
      "kind": "device",
      "device_id": "device-abc123",
      "label": "My MacBook"
    }
  }
}
```

`removed_message_id` is the event id of the `MessageReceived` the user removed from the queued follow-up list. The original `MessageReceived` remains in the append-only event log; this marker is metadata-only in the thread lifecycle projection, so it does not bump status, section, recency, or message count.

The frontend hides the matching exchange only while it has no steps. If a race already attached `UserPromptInjected` to that message, the exchange stays visible. The chat agentic loop also consults this marker before appending injected prompts, so a successfully removed queued prompt is not sent to the model.

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

The `description` is surfaced by `ThreadEvent::indexable_text()` and therefore **indexed into memory** like a message or response — a "what's this?" + screenshot turn would otherwise leave no trace in memory, since the typed `MessageReceived.text` carries none of the image's content. Note coding-agent threads never emit `ImageDescribed` (the description runs only in the chat agentic loop), so their image turns still index text-only.

### `ConversationSummarized`

```json
{
  "type": "ConversationSummarized",
  "data": {
    "summary": "Worked through the alignment pass on the edit timeline. The off-by-one on the splice boundary was fixed. Export presets were left as they are.",
    "covers_through_event_id": "550e8400-e29b-41d4-a716-446655440000",
    "covered_count": 31,
    "model": "gemini-3.5-flash"
  }
}
```

The **cache** for the older-turn summary, and the whole of it. There is no
table, so the paragraph survives an engine restart because the event does.

Before this event existed the paragraph was rebuilt on every turn. The
summariser lands on a minority of turns on a long thread, and each miss
rendered a bare "(N earlier messages not shown)" line. Now the first success
holds: a later failure reuses this paragraph instead of losing it.

`covers_through_event_id` addresses the newest assistant turn the paragraph
accounts for. `load_chat_history` compares it against the current older segment.
It re-summarises only when the assistant turns past that boundary exceed
`HISTORY_SUMMARY_REFRESH_AFTER`. Otherwise it reuses the paragraph and renders
the uncovered assistant turns compacted, so nothing between refreshes is
silently dropped.

**The refresh runs detached, so this event lands one turn late.** The turn that
decides one is owed has already chosen what it renders, from the cache as it
stood. Awaiting the call put its whole deadline in front of that turn's first
step, for a paragraph only the next turn could use.

**User turns are never summarised** (ADR 0102). They render verbatim in the
older region, so a constraint stated 40 turns ago is still in the prompt word
for word. This event therefore only ever stands in for assistant work.

**Every row covers its own thread** (ADR 0124). A chat turn reads only its own
events, so nothing here can file another conversation's content under this
thread.

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

- `CancelCause`: `user_stop` (Cancel button), `user_action` (Apply / Discard / Archive on running thread), `superseded_by_followup` (a follow-up interrupted a mid-turn **Codex** turn — the engine stopped it and ran the follow-up as the next turn, preserving partial work), `unknown` (legacy DB rows).
- `AbortCause`: `engine_shutdown`, `safety_net`, `recovery_after_restart`, `process_killed`, `stale_settle`, `session_dropped` (the run future was dropped instead of completed — its caller was cancelled, so the session went with it; emitted by the session entry's drop-guard), `unknown` (legacy).

On a *coding-agent thread*, `user_stop` is a **resumable turn boundary**, not a terminator: the `Cancel` button routes through the backend's native interrupt path, so the turn is interrupted but the session stays alive — `CodingAgentIdled` (with the `cc_session_id` when available) follows, the branch is kept, and the next message resumes the same conversation. This is distinct from `user_action` (Apply / Discard / Archive), which DO terminate via their own lifecycle event. See `system-knowhow/coding-agent-events.md` § `CodingAgentIdled` "Cancel = Esc".

`superseded_by_followup` is mechanically a cancel (no `ResponseGenerated`, no change proposal for the redirected-away partial work — the branch is kept and the follow-up turn's eventual proposal includes both turns' files) but the user **steered**, they didn't Stop. The frontend therefore renders it **neutrally** — the interrupted turn reads a plain "Done", with no "Canceled ✕" badge and no standalone "Response canceled" panel — exactly like a chat or Claude Code follow-up. Only Codex produces this cause (Claude Code steers a mid-turn follow-up via stdin and never cancels; idle-Codex follow-ups route via `turn/start`). See `docs/plans/2026-06-21-codex-followup-redirect-label.md`.

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
overflow, behind a leading `[truncated — N earlier bytes dropped, showing the
most recent M of T total]` marker. The end of a long build log is where the
failure is; `bash_output` renders the same tail when it falls back to this row,
so the live drain and the archived record can never disagree. `N` also counts
whatever the engine's ~2 MB per-stream ring buffer discarded while the task
ran, so it is the real gap rather than only the part trimmed at write time.

**Reading the status.** `exit_code` and `signal` are mutually exclusive, and neither is ever a stand-in for a status the engine didn't obtain:

| `exit_code` | `signal` | Meaning |
|---|---|---|
| `0` | absent | The command really exited 0. A reader can trust this. |
| non-zero | absent | Normal exit with that status. |
| `null` | set | The child was terminated by that Unix signal — `9` SIGKILL (the watchdog timeout and `bash_kill` both use it), `11` SIGSEGV, `13` SIGPIPE (a pipeline producer whose consumer closed the pipe). |
| `null` | absent | The engine could not determine the status. Treat as failure, never as success. Also the shape of rows written before `signal` existed. |

`signal` is omitted from the payload when there is none. So is `abandoned` when false.

**`abandoned: true` means the engine went away, not that the task failed.** A background task is a child of the engine process. A restart, a crash or an OOM ends it, and nobody is left to reap a status.

The engine records that itself, from two places. Graceful teardown kills each running task, waits for the reap, and writes the completion with everything the task ever output. The next boot then sweeps up whatever the teardown could not reach, which is every SIGKILL, OOM, panic and power cut. Both rows carry `abandoned: true`, no `exit_code` and no `signal`.

That is what keeps the promise in `system-knowhow/running-python.md` that ending a turn with background work running is a valid wait. The subscription resolves at the next boot, instead of sitting to its own deadline on an event that can never arrive.

**The two paths differ, and the `stderr` line says which one wrote the row.** Teardown killed the task and kept its output. The boot sweep kept neither: after a crash no destructor ran at all.

**Neither promises the work stopped**, and that is not hedging. A crash kills nothing. The teardown's SIGKILL reaches the task's own shell but not a pipeline behind it, so either can leave a child reparented to init. Check before re-running the same work.

`abandoned` is distinct from `killed`, which means `bash_kill` was called, and outranks it when the engine's own shutdown sent the signal. Reading one as the other says a person or an agent called the work off. One more thing follows from nobody having watched a crashed task exit: `finished_at` is when the loss was recorded, so on the boot path it spans the engine's downtime and is not the task's runtime.

**A failing pipeline stage is never masked by a later succeeding one.** The engine runs commands under `bash -o pipefail`, so `cargo clippy … 2>&1 | tee build.log` reports clippy's `101`, not `tee`'s `0`. Without `pipefail` a POSIX shell returns only the last stage's status, which silently turned failing builds into `exit_code: 0` — the defect this table's guarantees exist to prevent. Precisely, `pipefail` reports the *rightmost failing* stage (`sh -c 'exit 42' | sh -c 'exit 7'` → `7`), so it tells you a pipeline failed but not necessarily which stage when several can fail. On a host with no `bash` the engine falls back to `/bin/sh`, logs `[Shell] no bash found …`, and the guarantee does not hold.

Both events also fire for `run_python_background`. The `command` field then carries the venv-rooted python invocation, e.g. `'/<ws>/.lucidos/runtime/python/venv/bin/python' '/<ws>/.lucidos/exhaust/<run_id>/script.py'`, so the audit trail records which script ran (the file is preserved under `.lucidos/exhaust/`). One registry, one event pair, one watcher — chat-agent consumers don't branch on the spawning tool.

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

`producer` is `ContextProducer`, serialized snake_case: `main_llm`, `claude_code`, `codex`. Match those exact values in a `condition:` filter, not the PascalCase Rust variant names, which never match. `usage` is `None` pre-call and on providers that don't report it (OpenAI, Gemini); when present it carries the real provider-reported counts.

`context_window` is the model's window in tokens as the engine resolved it — the value declared on the model's *model registry* row, or, when that's unset, a guess from the model id (`[1m]`→1M, `claude-`→200k, `gpt-5`→400k, else 200k). A capture showing `200000` for a model you know is bigger means the row hasn't declared its *context window* yet, and the turn was budgeted against the smaller number.

`estimated_total_tokens` covers the system prompt, the tool definitions, and the messages: the whole request, matching what the trim budget accounts for. It is an estimate at a fixed 2.5 chars/token, measured across 12,069 captures against the real counts, so compare it to `usage.input_tokens` (the real total prompt) rather than treating it as exact. This is deliberately *not* the ratio the trim budget uses. The budget assumes a conservative 1.5 chars/token so it can never pack a prompt past the *context window*, while this number is read by a human and wants accuracy. It was 1.5 until 2026-08-07, which made every context readout run about 1.7x high. **A trigger condition on `estimated_total_tokens` written before that date wants re-scaling by 5/3**: the same prompt now reports about 0.6x what it used to, so a `{ $gt: 150000 }` threshold silently stops firing where it used to, rather than erroring.

Each section carries `name`, two sizes, and `role` (`system` / `prior_message` / `user`). It also carries an optional `group` label and an optional `content` body. The body is omitted when the `capture_context` preference is off, and truncated head and tail at 8,000 chars when it is on.

**The two sizes answer different questions, and picking the wrong one is the classic error here.**

- `budget_delta_chars` is what the section ADDS to the request, beyond what other sections already count. Sum this one. The LLM Context Viewer divides the headline total by that sum. So the section rows always add up to the number at the top of the panel. The headline is the measured `usage.input_tokens` when there is one, and the estimate otherwise.
- `content_chars` is the section's own size, measured whether or not the body was persisted. Ask this one how big a region was. Never sum it.

On almost every section the two are equal, because nothing else counts that section's chars. `Conversation` is the exception and the reason both fields exist: every other section is already concatenated into the first message, so its delta is only what the tool loop added on top. Summing its real size would count the whole bundle twice.

Both are character counts, never a token count. The sections carry no per-section token number.

**Pre-rename rows spell the delta `char_count`**, and carry no `content_chars` at all. (Not written as a `Legacy alias:` note: that form is reserved for a retired EVENT name, and this is a renamed payload key.) The engine renames the key on the way out of `GET /api/v1/events/:event_id/context`, so a client never sees it, but a direct SQL query over history does: read `coalesce(x->>'budget_delta_chars', x->>'char_count')`.

`trimmed` means the LLM was given less than the assembled context. It covers **both** ways that happens. The trimmer's removal pass evicts whole messages, and its stubbing passes replace an individual body with a note. The note opens `[cut to fit the context budget:` and states the original size. Where the body carried an event address, it also names the `events(action="query", event_id=...)` call that reads the whole thing back. It previously reported only the eviction case, so turns whose tool results had been gutted showed as untrimmed.

`trim_passes` says which passes did it, ascending, and is absent when nothing was trimmed. The distinction that matters is pass 5: it is the only one that removes a whole message, where every pass above it leaves an addressed stub the model can read back. A round with `trimmed: true` and no pass 5 lost nothing silently. A row written before this field existed carries no `trim_passes`, which reads as unknown rather than as none.

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

**Snapshot endpoint strips the heavy field.** Mirrors the `ContextCaptured` contract above: `GET /api/v1/threads/:thread_id/events` removes `result` from `ToolResult` payloads and stamps `result_stripped: true` so the events list stays small on busy tool-heavy threads (one bash result can be 150 kB+; a long session carries hundreds, totaling ~2 MB the chat exchange never renders — only `StepDetailModal.tsx`'s `<pre class="step-detail-result">` does). Live SSE emissions still carry the full text. The strip keeps `name` + `images` inline — the step row label and generated-image rendering paths in `thread-events.ts` need them. To fetch the dropped text on demand, call `GET /api/v1/events/:event_id/tool-result` — returns `{ result: string | null }` for that single `ToolResult` event (`null` for image-only results, which never had a textual result written). Same event-id-only routing as the context endpoint. The same `?include_context=true` flag on the snapshot endpoint that opts back into `ContextCaptured.sections` now ALSO opts back into `ToolResult.result` — covers `exportThread.ts` and any future bulk consumer. `CodingAgentToolResult` is NOT stripped today; if its bash-output bloat becomes the bottleneck, the same pattern (`strip_*_content` helper + `GET /api/v1/events/:event_id/...` endpoint + a marker field) applies.

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
    "branch_name": "lucidos-claude-code-repo-lucidos-add-threadevent-reference-doc",
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
    "actor": { "kind": "device", "device_id": "device-abc123", "label": "My MacBook" },
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

`status` is `success` / `failure` / `no_changes` / `canceled`. `summary` is truncated to 2000 chars. `pending_change_ids` is empty for chat children and for coding-agent children that ended without proposing anything.

**The parent is re-opened BY this callback, so it never has to wait for one.** The fan-in persists it on the parent and re-opens that thread with the same status / summary / `pending_change_ids` an *event wait* would have delivered. That makes an `await_event` (or `lucidos await-event`) subscription on your own child's completion redundant: the engine stands the fan-in callback down when a live wait already covers it, so it is one turn either way, but the subscription still spends part of the consecutive-subscription budget and arms a timeout that can fire while the child is still working. Awaiting a `ChildThreadCompleted` is right only for a completion that is not the awaiting thread's own child's, named with a `child_thread_id` condition. Matching is workspace-wide, so that is any thread's child and not only a descendant of the awaiting thread's: the card is persisted on whichever thread is the parent, and the wait resolves off that row wherever it lands.

**One callback per completed turn, not one per child.** A child can report more than once: a parent that sends a *child follow-up* revives or redirects the child, and that turn's own terminal produces a second `ChildThreadCompleted` for the same `child_thread_id`, on the same parent. A human clicking Continue on a coding-agent child does the same. So do not treat `child_thread_id` as a key; the events are a log of completed turns.

**A steer is not a completion.** A `ResponseCanceled` whose cause is `superseded_by_followup` is the mid-turn redirect the engine arms when a follow-up lands on a live Codex turn: the caller steered, they did not abandon, and the child runs the redirected turn immediately afterwards. It fires no `ChildThreadCompleted` and no parent callback. The redirected turn's own terminal is the report.

**Running more than one child at a time: `system-knowhow/orchestrating-sub-threads.md`.** This edge and the *child follow-up* below are the only two carrying traffic between threads, and nothing carries it sideways. That file is the operating manual for a parent coordinating several children. It covers what a child may do about a sibling's events, and how a ruling reaches a child that already finished.

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
    "branch": "lucidos-claude-code-repo-lucidos-…",
    "origin": { "kind": "engine", "reason": { "kind": "continuation_started" } },
    "reason": "auto_recovery_after_hang"
  }
}
```

`origin.reason.kind` is `continuation_started` (legacy alias `session_recovered`).

`reason` (optional) mirrors the originating `ContinuationRequested.reason` so the
timeline can label the resume honestly, naming which interruption it recovered
from. `user_clicked_continue` is a genuine resume after an engine restart (the
user tapped "continue"). `auto_recovery_after_hang` fires for a hung subprocess
OR a stray signal-kill where **nothing restarted** (e.g. another workspace's
`cargo check` build-lock kill landing on this coding agent's process), and the UI
labels it "Resumed after the session stopped responding".
`auto_resume_after_api_error` is the engine picking a turn back up after a
transient upstream failure the agent reported itself, labelled "Resumed after the
model connection dropped". Neither of the two auto reasons claims "Resumed after
engine restart", and the two are worded apart because both can fire on one thread
minutes apart. Absent on legacy rows and the chat-rerun path.

**The boundary confers no thread type.** `ContinuationStarted` is emitted on all
three channels — `chat` and `trigger` from `emit_resume_anchor` (reached from
`POST /api/v1/threads/<id>/continue` and the chat answer-resume path), and
`claude_code` from the coding agent's `--resume` dispatch. A thread is a
*coding-agent thread* because a `SessionStarted` opened an agent session on it,
never because it was continued. The `thread_summaries` projection reflects that:
it sets `is_coding_agent` (and rewrites `source`) for `SessionStarted`
unconditionally, but for `ContinuationStarted` only on the `claude_code` channel.
It also never *clears* the flag — repairing rows an earlier unconditional write
corrupted is a migration's job, not the projection's.

**Only an interrupted turn gets this boundary.** A thread parked on an
unanswered `UserQuestionAsked` is *preserved* across a restart (no
`ResponseAborted` is emitted), so answering it emits **no** `ContinuationStarted`
either: the resumed work carries the original turn's `request_event_id` and
continues that exchange, exactly as it would have without the restart. A
subscription on `ContinuationStarted` therefore fires for a revived
interruption, never for an answered question. "Parked" is strict: it holds only
while the question is still the newest thing on the thread. If anything in
`ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES` landed after it the agent has moved
on, the card is dead, and the thread recovers as an ordinary interrupted turn
with its boundary and its Continue button. See `coding-agent-events.md`
§ "An engine restart alone does NOT orphan a pending question".

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

Four knobs:

1. **Pick the right event.** Lifecycle / one-per-turn variants are usually what you want. Per-action variants such as `ToolCalled`, `CodingAgentToolCalled`, `ContextCaptured` and `ImageDescribed` fire many times per turn. Always pair them with the entry's `condition:` filter, so the trigger matches only the case you care about: `name: "Bash"`, or `estimated_total_tokens: { $gt: 150000 }`. A key is a field path, so a nested field is reachable: see § "Volume classes".
2. **Per-token streaming is off-limits.** `TextStreamed` / `ThoughtStreamed` / `CodingAgentTextStreamed` are blocked at the scheduler, and the create is refused rather than armed. A workspace that genuinely needs token-level reactivity consumes the SSE stream directly, not a trigger.
3. **For workspace-defined signals**, `lucidos events emit` (or the `emit_event` LLM tool) writes a `SystemEvent::DomainEvent` that flows through the matcher unconditionally. Use this when you want a name that isn't part of the engine's own ThreadEvent enum (e.g. `OuraDataImported`, `BuildBroken`) — see `system-knowhow/lucidos-cli.md`.
4. **For workspace-scoped engine facts**, subscribe to the persisted `SystemEvent` directly. `BackupFailed`, `NotificationCreated` and `TriggerCompleted` are `on:` entries like any other. Do not emit a domain event beside one to make it reachable: the engine already wrote the row.

Trigger-run failures still auto-create an error notification — no separate wiring needed for "tell me when one of my own triggers blew up."

## Recipe-shaped guidance

For trigger config syntax (cron format, the `on` subscription list, the per-entry `condition` operator vocabulary), see `system-knowhow/triggers.md`. Conditions are pure payload filters: each key is a field path into the event payload (the `data: { … }` object above).

For the coding-agent slice — the `UserQuestion` vs permission distinction, the exact `CodingAgentIdled` field semantics, and the no-`CodingAgentErrored` gap — see `system-knowhow/coding-agent-events.md`.

For event-store column shape (`event_type`, `payload`, `created`, `aggregate`, `aggregate_id`, `sequence`) and the queries used to walk threads from events, see `.claude/rules/db.md`.
