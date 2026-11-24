---
name: Coding Agent Lifecycle Events, Triggerability: blocklist semantics
description: Part of Coding Agent Lifecycle Events. Its index lists the sections it covers.
---

One part of `system-knowhow/coding-agent-events`, which lists every part.

## Triggerability: blocklist semantics

The scheduler forwards persisted ThreadEvents to the trigger matcher, unless they sit on a small **blocklist**. That is `core::event_subscription::is_subscribable`, called at the `BusEvent::Thread` arm of the scheduler subscriber in `crates/lucidos-engine/src/scheduler/mod.rs`. It drops the per-token streaming variants (`ThreadEvent::is_per_token_streaming`) and the side-question events (`ThreadEvent::is_side_question_event`). Two coding-agent streaming entries sit on it, `CodingAgentTextStreamed` and `CodingAgentThoughtStreamed`, beside the four side-question events.

So, for each entry in a trigger's `on` list (full shape in `system-knowhow/triggers.md`):

- `event_type: UserQuestionAsked`: works. Have the trigger call `send_notification` with `tap: { kind: 'navigate', to: { target: 'thread', id: '<thread_id>', event_id: '<source_event_id>' } }` to deep-link the push straight to the question. Take `event_id` from the trigger's `Source event id:` line.
- `event_type: CodingAgentIdled`: **works**. Add `condition: { has_changes: true }` to scope to "the turn ended with work on the branch". That work may be unproposed: a Stop, a failure or a plan hold leaves it there too. For "a change is ready to review", subscribe to `ChangeProposed` instead.
- `event_type: ProposalWithheld`: **works**. Reacts to "a turn ended and left work it did not propose", with the `reason`.
- `event_type: CodingAgentPermissionRequest`: **works**. Reacts to "the coding agent is asking permission for a tool call."
- `event_type: CodingAgentToolCalled` / `CodingAgentToolResult` / `CodingAgentPromptSent`: **works**, but these are per-action and chatty. Always add a `condition:` (e.g. `name: "Bash"`) or the trigger fires many times per turn. A condition key is a field path, so the command inside `args` is filterable too: `{ "args.command": { "$regex": "cargo test" } }`.
- `event_type: CodingAgentTextStreamed` / `CodingAgentThoughtStreamed`: does not fire. The scheduler blocks per-token streaming, so subscribing to either is a no-op.
- `event_type: SideQuestionAsked` (or any side-question event): refused. No agent, trigger or event wait may see a side question (ADR 0320).
- `event_type: <any chat-side lifecycle event>` (`ResponseGenerated`, `ResponseFailed`, `ChangeApplied`, …): works, same blocklist. See `system-knowhow/thread-events.md` for the full set.

The blocklist is not the only gate. A trigger is never woken by an event its own fire emitted, so an *intent* trigger subscribed to `ResponseGenerated` does not see its own. But a coding-agent session the fire STARTS runs on its own thread, so its `CodingAgentIdled` still wakes the trigger. That is what makes "wait for the session I started" work.

Across triggers, `max_event_trigger_depth` (default 5) bounds the chain, and that gate DOES follow the handed-off session. Every event the session emits carries the fire's own depth. So the waiting trigger fires at hop 1, while a chain that keeps going ends at the ceiling. See `system-knowhow/thread-events/one-table-two-enums.md` § "Today the scheduler uses a blocklist" for both gates, and `system-knowhow/triggers.md` for what they mean when you author a subscription.

