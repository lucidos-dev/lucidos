---
name: ThreadEvent Reference
description: Every `ThreadEvent`, by family (chat, coding-agent, lifecycle, changes, background bash, plugin, repo, merge conflict, transient SSE-only). With payload, persistence, and whether a trigger can subscribe. Load for "what events fire on a thread" or "can I trigger on ResponseGenerated / ChangeApplied / BackgroundBashCompleted". Also for "is X persisted" and "is X a real event name".
---

<!-- domain-event: BuildBroken: Example domain event emitted by a build-monitoring script. -->

<!-- domain-event: HabitCompleted: Example domain event emitted by a habit-tracker app. -->
<!-- domain-event: OuraDataImported: Example domain event emitted by a wearable-data integration. -->
<!-- domain-event: LucidosReleased: Release automation announces a published version. Optional `commits` lists source main-line merges since the previous release. -->

# ThreadEvent Reference

Every `ThreadEvent`: the per-thread event family that flows through the EventBus into PostgreSQL, the SSE stream and the trigger matcher. Source of truth: `crates/lucidos-core/src/engine/thread_events/` (the enum is in `event.rs`). Names below are the **current** names. Legacy aliases (e.g. `ClaudeCodeIdled`, `SessionRecovered`, `parent_thread`, `task_id` / `task_name`) exist as `#[serde(alias = ...)]` so old DB rows decode. Write new code, triggers and docs against the current name only.

This file is the master enumeration. The coding-agent slice (`CodingAgent*` and the `UserQuestion*` / permission machinery) is summarized here, with the deep-dive in `system-knowhow/coding-agent-events.md`.

Event-store column shape, the chat-mode terminator set and the `events` table schema: `.claude/rules/db.md`. Trigger config syntax (cron, the `on` list, per-entry `condition` operators): `system-knowhow/triggers.md`.

The command permission event may also carry optional `decision_trace` text.
It records the deciding layer, static evidence and judge distribution without command text.
It does not change triggerability.

Thread liveness is a derived read, not an event or subscription name. Its handoff eligibility follows the same start rules as successor sessions.
An eligible automatic successor keeps the thread `running` and in In flight through the old session's idle/end and successor startup. This status does not prove a process still owns the thread. After the old turn ends, an owed successor that misses the start grace reads `stalled`; an active silent turn reads `quiet`.
Stop, failure, open questions, a hold, next-input starts and closed-lane exclusions prevent an automatic transition.
A new handoff requested after a failed turn can start fresh. Stop between sessions cancels the pending handoff, just as Stop during a live turn does.
Liveness observes the wait lists that drive its `waiting` state in the same
read. The event waits come from the projection of `EventWaitStarted` and its
closing events. The `expected_wake` field on a waiting or held thread comes
from the newest `WakeExpected` whose event sequence is after the last turn
start. See `system-knowhow/glossary/core-terms-10.md` § Thread liveness for the states and
the evidence each reader sees.

## Parts

This doc is split into parts so each one fits a coding agent's tool output. Load the part that covers what you need.

| Part | Covers |
|---|---|
| `system-knowhow/thread-events/one-table-two-enums` | One table, two enums; Today the scheduler uses a blocklist (Triggerable is not the same question as awaitable); Check the name before you subscribe (The path is checked too, not just the name); Persisted vs transient |
| `system-knowhow/thread-events/wire-format-and-metadata` | Wire format and metadata (Saved-draft origins: `origin.kind == "saved_draft"`; Another workspace: `origin.kind == "workspace"`; Engine origins: `origin.kind == "engine"`); Volume classes; Chat / agentic loop |
| `system-knowhow/thread-events/chat-agentic-loop` | Chat / agentic loop (continued) |
| `system-knowhow/thread-events/chat-agentic-loop-2` | Resume / continuation; Coding agent (Claude Code / Codex) |
| `system-knowhow/thread-events/coding-agent-claude-code-codex` | Coding agent (Claude Code / Codex) (continued) |
| `system-knowhow/thread-events/coding-agent-claude-code-codex-2` | Question / permission machinery |
| `system-knowhow/thread-events/form-requests` | Form requests; Thread lifecycle |
| `system-knowhow/thread-events/thread-lifecycle` | Thread lifecycle (continued) (Deleting a thread is a `engine::event_bus::SystemEvent`, not one of these; Correcting a memory is a `engine::event_bus::SystemEvent` too; A dashboard reports as `engine::event_bus::SystemEvent`s; Freeing disk space reports as `engine::event_bus::SystemEvent`s); Planning stages |
| `system-knowhow/thread-events/changes-per-thread-coding-agent-change-p` | Changes (per-thread coding-agent change proposals); Cross-thread / context |
| `system-knowhow/thread-events/event-wait-a-thread-holding-a-subscripti` | Event wait (a thread holding a subscription) (A subscription does not hold the turn; The re-entry anchor; Both agents, one registration; Limits; The cache keep-alive status; The expected wake; The ping plan) |
| `system-knowhow/thread-events/voice-session-a-thread-being-spoken-to` | Voice session (a thread being spoken to); Which call each purpose names; Transient: never persisted, broadcast over SSE only; Indexable text; Concrete payload shapes (selected) |
| `system-knowhow/thread-events/concrete-payload-shapes-selected` | Concrete payload shapes (selected) (continued) (`MessageReceived`; `QueuedMessageRemoved`; `ImageDescribed`; `ConversationSummarized`; `ResponseGenerated`; `ResponseCanceled` / `ResponseAborted` / `ResponseFailed`) |
| `system-knowhow/thread-events/concrete-payload-shapes-selected-2` | Concrete payload shapes (selected) (continued) (`BackgroundBashStarted` / `BackgroundBashCompleted`; `ContextCaptured`; `ModelCallFailed`; Provider spend-cap holds) |
| `system-knowhow/thread-events/concrete-payload-shapes-selected-3` | Concrete payload shapes (selected) (continued) (`ToolResult`; Plan and implementation stages; `ChangeProposed`; `ProposalWithheld`; `ChangeWithdrawn`; `ChangeApplied`; `ChildThreadCompleted`; `ChildReportFiled`; `ChildThreadStopped`; `ChildThreadDetached`) |
| `system-knowhow/thread-events/concrete-payload-shapes-selected-4` | Concrete payload shapes (selected) (continued) (`TriggerStarted` / `TriggerCompleted`; `TriggerRunInterrupted`; `WorktreeCleaned`; `ContinuationStarted`); How a workspace would actually trigger on these; Recipe-shaped guidance (Workspace snapshot events) |
