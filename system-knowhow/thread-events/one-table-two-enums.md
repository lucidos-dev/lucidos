---
name: ThreadEvent Reference, One table, two enums
description: Part of ThreadEvent Reference. Its index lists the sections it covers.
---

<!-- domain-event: HabitCompleted: Example domain event emitted by a habit-tracker app. -->
<!-- domain-event: OuraDataImported: Example domain event emitted by a wearable-data integration. -->
<!-- domain-event: LucidosReleased: Release automation announces a published version. Optional `commits` lists source main-line merges since the previous release. -->

One part of `system-knowhow/thread-events`, which lists every part.

## One table, two enums

**This is the canonical statement of the `ThreadEvent` / `engine::event_bus::SystemEvent` split. Everything else points here.**

There are two Rust enums and **one** table.

- **`ThreadEvent`** (`crates/lucidos-core/src/engine/thread_events/`) is the per-thread family in this file: `MessageReceived`, `ResponseGenerated`, `CodingAgentIdled`, `ChildThreadCompleted`, every `Change*`, every `Thread*` lifecycle event.
- **`engine::event_bus::SystemEvent`** (`crates/lucidos-core/src/engine/event_bus_system_event.rs`) is the workspace-scoped family: `NotificationCreated`, `TriggerCompleted`, `PluginInstalled`, `PreferencesChanged`, and so on. Its **`SystemEvent::DomainEvent`** variant carries names the workspace invents (`HabitCompleted`, `OuraDataImported`, `LucidosReleased`). Writers: `lucidos events emit`, the `events` LLM tool's `emit` action, `lucidos.events.emit` in an app, or `POST /api/v1/events/emit`.

Both go through the one `EventBus::emit`. Every variant whose `is_persisted` method returns true goes through the one `EventBus::persist` INSERT into the one **`events`** table, with `event_type` set to the variant name. A `DomainEvent` is stored under its *inner* type (`HabitCompleted`), never the literal string `"DomainEvent"`.

Only two columns differ:

| | `aggregate` | `aggregate_id` | `thread_id` column |
|---|---|---|---|
| `ThreadEvent` | `'thread'` | the thread id | set (mirrored from `aggregate_id`) |
| `SystemEvent::DomainEvent` | `'domain'` | the event type | NULL |
| other `engine::event_bus::SystemEvent` | its own (`'notification'`, `'trigger'`, `'plugin'`, `'app'`, …) | the entity id | NULL |

The rest of both enums is **transient**: a `ThreadEvent` whose `ThreadEvent::is_persisted()` is false (see "Persisted vs transient"), or a `DomainEvent` emitted with `{ transient: true }`. These go out on SSE and are never written, so nobody can query them. A transient `ThreadEvent` cannot fire a trigger. A transient `DomainEvent` can, live only: under heavy load a fire can be missed, because there is no row to read back (`system-knowhow/triggers.md`).

**Consequence for reading.** `GET /api/v1/events/query`, `GET /api/v1/events/count`, `GET /api/v1/events/types`, `lucidos events query`, `lucidos.events.query` in an app, and the `events` LLM tool all run one statement over that table with **no aggregate predicate**. They return persisted rows of BOTH enums, filtered by `event_type` and time. All of them leave out the four side-question events (ADR 0320). There is no separate domain-event stream to "switch to".

The case people get wrong: **`ChildThreadCompleted` is queryable by an app today.** It is a persisted `ThreadEvent`, emitted on the **parent** thread by EventBus fan-in. So the row's `thread_id` is the parent, and the payload carries `child_thread_id` / `child_thread_title` / `status` / `summary`. `lucidos.events.query({ event_type: 'ChildThreadCompleted' })` returns it with no engine change.

So the split is a Rust type distinction and a column value, never a storage boundary.

## Today the scheduler uses a blocklist

The scheduler forwards EventBus events to the trigger matcher. Each enum has
its own gate. The `BusEvent::Thread` branch calls
`core::event_subscription::is_subscribable` from
`crates/lucidos-engine/src/scheduler/mod.rs`. It drops three groups:

- **Per-token streaming** (`ThreadEvent::is_per_token_streaming`): `TextStreamed`, `ThoughtStreamed`, `CodingAgentTextStreamed`, `CodingAgentThoughtStreamed`. Each fires once per text chunk, so a trigger never belongs on one.
- **Side questions** (`ThreadEvent::is_side_question_event`): `SideQuestionAsked`, `SideQuestionAnswered`, `SideQuestionFailed`, `SideQuestionDismissed`. They record a card no agent may see, so a subscription on one is refused (ADR 0320).

- **Engine turn idles** (`ThreadEvent::is_engine_turn_idle`):
  `CodingAgentIdled` carrying the reason `cache_keep_alive` or
  `ceiling_reentry`. These record no work the user asked for.

Every other persisted `ThreadEvent` reaches the matcher and is a valid `on:` entry.

The `BusEvent::System` branch is an **allowlist**: **a persisted `engine::event_bus::SystemEvent` is subscribable, a transient one is not** (ADR 0113). It admits every variant whose `SystemEvent::is_persisted()` is true, plus a `DomainEvent` on either setting of `transient`. So `BackupCompleted`, `BackupFailed`, `NotificationCreated`, `TriggerCompleted`, `PluginInstalled` and the rest of the persisted set are valid `on:` entries. `BackupProgress`, `Toast`, `MemoryRebuildProgress`, `RecoveryProgress`, `EmbeddingModelStatusChanged` and `TreeBackfillProgressed` are transient frames that write no row, so they reach neither matcher. `DiagnosticBundleReceived` is also persisted and triggerable; its payload names the private inbox receipt without including its contents. The full persisted list is `SystemEvent::PERSISTED_TYPE_NAMES`, and `.claude/rules/db.md` § Key event types enumerates the variants.

Both fan-outs run one predicate, `core::event_subscription::is_subscribable_system_event`, so a trigger and an `await_event` are offered the same set.

High-cardinality per-action variants (`ToolCalled`, `ToolResult`, `CodingAgentToolCalled`, `CodingAgentToolResult`, `ContextCaptured`, `MemoryRecalled`, `ImageDescribed`, `PromptInjected`, `CodingAgentPromptSent`) are **triggerable**. Scope them with per-entry `condition:` filters (e.g. `name: "Bash"`, `estimated_total_tokens: { $gt: 150000 }`). A condition key is a **field path**, so `{ "args.command": { "$regex": "cargo test" } }` reads one level down. Operators are `$eq` / `$ne` / `$lt` / `$lte` / `$gt` / `$gte` / `$in` / `$nin` / `$regex`, and a bare value means `$eq`. `$or` in key position takes a list of conditions.

**`thread_id` is available on every event in this file, and on none of their payloads.** The engine supplies it at matching time from the event's own thread. So `condition: { thread_id: "<uuid>" }` scopes any event type to one thread: one session's `CodingAgentIdled`, one thread's next `ResponseGenerated`, one thread's next tool call. It never appears in a payload read back with `events(action="query")`, where the row's thread column carries it. A **`engine::event_bus::SystemEvent`** belongs to no thread, so a domain event or `BackupCompleted` has no `thread_id` to filter on.

What works today (each line is one entry in a trigger's `on` list; full shape in `system-knowhow/triggers.md`):

- `event_type: UserQuestionAsked`: works. Typical use: "push me when a question is raised so I can answer from my phone." Pair `send_notification` with `tap: { kind: 'navigate', to: { target: 'thread', id: '<thread_id>', event_id: '<source_event_id>' } }` to deep-link to the question (worked example in `triggers.md`).
- `event_type: CodingAgentPermissionRequest` / `CommandPermissionRequested` / `McpPermissionRequested`: **work**. The other blocking requests that should wake the user. `CommandPermissionRequested` is the chat command-guard card (ADR 0002); `McpPermissionRequested` is the chat MCP-tool card.
- `event_type: CredentialRequested` / `PluginInstallRequested` / `PluginUninstallRequested` / `EmailConfirmRequested` / `OAuthAuthorizationRequested`: **work**. These are *form requests*. Pair one with `FormRequestResolved` (same `request_id`) to learn how it ended. See § Form requests.
- `event_type: ResponseGenerated` / `ResponseFailed` / `CodingAgentIdled` / `ChangeApplied` / `ChangeHardened` / `TriggerCompleted` / `BackgroundBashCompleted` / every `Change*` / every `Thread*` lifecycle event: **work**.
- `event_type: ToolCalled` / `CodingAgentToolCalled` / `ContextCaptured` / `ImageDescribed` etc.: **work**, scoped with a `condition:`.
- `event_type: TextStreamed` and the other per-token variants: **refused**, at both surfaces.
- `event_type: <a workspace-emitted DomainEvent>`: works. `SystemEvent::DomainEvent` (from `lucidos events emit` / the `events(action="emit")` LLM tool) always reaches the matcher. This is the supported path for "trigger on something my workspace observes."
- `event_type: BackupCompleted` or any other persisted `engine::event_bus::SystemEvent`: **work**, scoped with the payload's own fields.
- `event_type: BackupProgress` or another transient frame: **refused**, at both surfaces. The refusal names the persisted event that ends the run, e.g. `BackupCompleted` or `BackupFailed`.

**Two more gates apply to every carrier.** First, self-exclusion. Every event a trigger's fire emits carries that trigger's id, and the matcher drops that trigger from the matches. So a trigger never wakes on its own fire, at any depth. Every other subscriber still sees the event.

Second, the recursion cap bounds a chain ACROSS triggers. An event a fire emits dispatches one level deeper than the event that fired it. Past `max_event_trigger_depth` (a *capacity policy* field, default 5) the event is still stored and reaches SSE, but fires no further triggers. An ordinary turn dispatches at depth 0.

**The two gates part company at a spawn.** Work a fire hands off emits UNMARKED, so a sub-thread or coding-agent session it starts still wakes the trigger. That makes "wait for the session I started" work. The depth does reach that work: a sub-thread (`run_thread`), a coding agent (`run_coding_agent`) and a script all run at the fire's OWN depth. A hop is a trigger fire, so a spawn does not buy a fresh chain.

**When the cap stops a fire, you get a notification.** It names the trigger, the event and the ceiling, once per trigger per ten minutes. Raise `max_event_trigger_depth` in the Thread Queue panel if a chain is legitimately longer. `system-knowhow/triggers.md` covers what the two gates mean for authors.

A new per-token streaming variant goes into `ThreadEvent::is_per_token_streaming` in the same change. Other `ThreadEvent` variants need no scheduler change. Neither does a new `engine::event_bus::SystemEvent`: being persisted is what makes it subscribable.

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

