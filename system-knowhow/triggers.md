---
name: Triggers
description: Load for requests to automate work ("every morning", "notify me when X happens") or to run an existing trigger now. Also load for "help me review my trigger grants". It routes "tell me HERE when X happens" to `await_event`.
---

<!-- domain-event: OrderPlaced: Example domain event emitted by an order-entry app. -->
<!-- domain-event: OrderQueuedForShipping: Example domain event emitted by an order-processing trigger. -->

<!-- domain-event: OuraSleepImported: Example domain event emitted by a wearable sleep-data integration. -->

<!-- domain-event: EmailReceived: Example domain event emitted by an email-ingestion integration. -->

# Triggers

The working reference for *triggers*: choosing one, building it, editing it, and running an existing one off-schedule. The engine system prompt and the grouped `triggers` tool description carry the cron format and field reference, so this file does not restate them. `docs/taxonomy.md` § Triggers has the intent-versus-procedure worked example.

> **Tool surface.** The grouped **`triggers`** tool (`action: create | list |
> update | delete | pause | resume | run`) and the grouped **`trigger_groups`**
> tool (`action: list | create | rename | reorder | delete`) manage triggers.
> Examples use grouped calls such as `triggers(action="update", trigger_id, …)`.
> The old flat names remain legacy aliases; the model sees the grouped tools.
> The CLI exposes
> `lucidos triggers …` / `lucidos trigger-groups …` (see `lucidos-cli.md`).
> Pause or resume through the CLI with `lucidos triggers update --paused true`
> or `--paused false`; the separate `pause` and `resume` actions are LLM-only.

## Parts

This doc is split into parts so each one fits a coding agent's tool output. Load the part that covers what you need.

| Part | Covers |
|---|---|
| `system-knowhow/triggers/when-a-trigger-is-the-right-answer` | When a trigger is the right answer (First ask where the answer goes, not just how often); Write the knowhow file FIRST, then the intent (The test to apply to your own draft; Worked example); Cron vs. `on` vs. both (One trigger, multiple events; Aggregating events: cron, per event, or a projection) |
| `system-knowhow/triggers/writing-cron-expressions` | Writing cron expressions (The footgun: one expression is not "either"; Day-of-week numbering; nth weekday of the month; Last weekday of the month; Expressions that can never fire; A fire due while the engine restarts; Skipped slots); `condition`: when to filter (Who emitted a domain event: `actor`; What a condition can say; The envelope: a webhook's body lands under `payload`); Continue key |
| `system-knowhow/triggers/actor` | Actor; Notification discipline; Where the thread lands: `go_to_review`; Which model the run uses: `model` and `reasoning_effort` (Which backend serves the model: `provider`; Notification routing (`app_id`, `tap`, `event_id`)); Script triggers: when an LLM call is overkill (Scripts run in place: `__file__` is the real path; Script trigger env vars; Worked example: push when any subscribed event fires) |
| `system-knowhow/triggers/grouping-triggers` | Grouping triggers; Program grants (Trigger denies; Coding-agent grant; Context ceiling); MCP servers the fires use: `mcp_servers`; Edit, don't recreate; Know-how a plugin removed |
| `system-knowhow/triggers/running-an-existing-trigger-once-off-sch` | Running an existing trigger once, off-schedule (Event-only trigger: emit the event; Don't imitate the fire); What a trigger fire may not do to other triggers (What to do instead); Questions to settle with the user before creating; One-shot triggers; On-disk trigger definition (`trigger.toml`) (Renamed trigger → stale `run.path`); Setup checklist; Common mistakes to avoid |
