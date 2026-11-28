---
name: Triggers, When a trigger is the right answer
description: Part of Triggers. Its index lists the sections it covers.
---

<!-- domain-event: OuraSleepImported: Example domain event emitted by a wearable sleep-data integration. -->
<!-- domain-event: EmailReceived: Example domain event emitted by an email-ingestion integration. -->

One part of `system-knowhow/triggers`, which lists every part.

## When a trigger is the right answer

| User says | Right answer |
|---|---|
| "Every morning, send me…" | Trigger (cron) |
| "Notify me when my package ships" | Trigger (`on`), with a separate event-emitting source |
| "When either X or Y happens, do Z" | One trigger with multiple entries in `on`, not two parallel triggers |
| "Check this once and tell me" | Just do it now, no trigger |
| "Remind me at 5pm today" | One-shot trigger (cron for today). See "One-shot triggers" below |
| "Turn trigger X back on afterwards" / "re-enable X tomorrow" | A trigger cannot re-arm another trigger. Use a skip marker, or a human re-arm. See "What a trigger fire may not do to other triggers" |
| "Tell me **here** when X happens" | `await_event`, NOT a trigger. See the next section |

### First ask where the answer goes, not just how often

Both answers are the same *event subscription* underneath: `{event_type,
condition}`, matched by the same code. They differ in who consumes the match. A
**trigger subscription** spawns a new thread and stays armed for the next one. A
**thread subscription** resumes an existing thread and is spent once it fires.

A trigger runs in **its own thread**. It reaches the user as a notification and
cannot continue the conversation they are typing in. So a request like "let me
know **here** when a coding agent edits code" is a thread subscription
(`await_event`), not a trigger. So is "tell me in this chat when the build
finishes", or any request made inside a thread the user is plainly waiting in.
This holds **even when the phrasing sounds like a standing rule**: the user
asked for delivery into this conversation, and only `await_event` does that.

`await_event` costs nothing while it watches. The call returns at once, the turn
ends normally, and the engine re-opens the thread when the event lands. It is
one-shot, so you re-arm per event. The tool description carries the re-arm cap,
which is why an unbounded promise belongs to a trigger.

Duration is the *second* question, and the rest of this file is about it. A
reaction that must outlive the conversation, run when nobody is present, and
fire indefinitely is a trigger. Often both answers are right. Lead with the one
that matches where they asked to be told, and offer the other: "I'll watch and
report here; want a trigger too, so it keeps running once this thread is done?"

Do **not** build a trigger whose job is to post back into a chat thread. That is
the workaround `await_event` replaced (`docs/adr/0047-event-wait-is-an-event.md`).
It costs a persisted trigger row per wait, orphaned if the thread dies, plus an
extra LLM turn in a *different* thread. What lands is a fresh message that
starts a **new exchange**, and the waiting thread reads as finished.

A one-off for **right now** (a check, a lookup, a computation) needs no trigger:
do it inline. A one-off anchored to a **future time** ("remind me at 5pm") needs
a **one-shot trigger**, ideally self-deleting. See "One-shot triggers" below.

## Write the knowhow file FIRST, then the intent

This is the rule most often got wrong, so it is an **ordering**, not a
prohibition. "Don't put procedure in `run.intent`" says what not to write, and a
model holding a procedure with nowhere else to put it writes it anyway.

> **Write the procedure to a knowhow file BEFORE you create the trigger. Then
> write `run.intent` in the user's voice, with none of the procedure in it.**
> Procedure means anything about how: a script to run, a flag to pass, a format
> to follow, a threshold to compute, a fallback, a file to read first.

The system prompt's intent registry advertises what knowhow exists. The trigger
thread calls `load_knowhow` at fire time when it judges a recipe relevant,
exactly as a chat session does. There is no per-trigger allow-list and nothing
to wire: a file with a precise `name` and `description` is the whole mechanism.
Placement rules are in `building-knowhow.md`.

**A trigger-scoped file is the one you cannot write first.** Its path is
`data/triggers/<slug>/knowhow/`, and the tools take no `slug`, so the
authoritative slug exists only after creation. A guessed slug strands the file
where no thread of that trigger reads it. So put the recipe in shared
`data/knowhow/`, its right home unless nothing else could use it. If it is truly
private to this trigger, create the trigger and read the slug back from the
list. Then write the file before you reply.

### The test to apply to your own draft

Read each sentence of the intent and ask: **would deleting it change HOW the
work gets done, or WHAT the user wants?** How belongs in knowhow. What stays.

### Worked example

The user says: *"Set this up to run on its own every morning, and notify me when
the failure rate goes over the threshold we agreed."*

**Bad**, one step, everything inline:

```
intent: "Every morning at six, write the Build Health report for example-repo
         from the BuildObserved events of the previous day, following the
         conventions in artifacts/build-health/conventions.md. Work out the
         failure rate as a percentage with one decimal in Europe/Oslo time. If
         it is over 25%, tell me. Never notify between 22:00 and 07:00, so an
         alert that would land at six waits for the seven o'clock run."
```

Every clause after the first is procedure in the user's voice. The next thread
that needs the recipe rediscovers it, and a reader of the trigger config cannot
tell what was asked for.

**Good**, two steps. First the knowhow file, in shared
`data/knowhow/build-health-report.md`. You can write it before the trigger
exists, and another thread may want the recipe:

```markdown
---
name: Build Health daily report recipe
description: How the daily Build Health report is produced: the collector, the
  rate calculation, the report format and the quiet-hours hold. Load when
  writing or debugging the Build Health daily report or its trigger.
---

- Collect with `collect.py --offline <project>`. `collect.sh` always fails.
- Rate = failures / builds, one decimal, Europe/Oslo. Alert over 25%.
- Report file is `YYYY-MM-DD-health.md`; no table wider than four columns.
- Quiet hours 22:00 to 07:00: hold a would-be alert until 07:00.
```

Then the trigger:

```
intent: "Every morning, write the Build Health report and tell me if the failure
         rate is over the threshold. Stay quiet during my quiet hours."
```

Two sentences, both things the user would say. Delete either and what they want
changes. Delete any line of the knowhow file and only the how changes.

The HTTP API accepts a procedure-laden intent and will not stop you.

## Cron vs. `on` vs. both

- **Cron**: "every morning at 8" / "weekdays at noon". Time-driven.
- **`on`**: "when X happens". Reactive. Each entry in the `on` array is a *trigger subscription*: an event type plus an optional payload filter. The event must already be emitted by something (an app, another trigger, an integration). A match spawns a new *trigger thread* and leaves the subscription armed for the next one, which is what makes a trigger a standing rule.
- **Both**: rare. It usually means a cron with a payload-shaped condition that should be event-driven. Re-examine before doing this.

If the user says "notify me when X" and X isn't an event yet, you have two jobs: (1) make X emit an event, (2) trigger on it. Tell the user that explicitly.

**Check first whether the engine already emits it.** An `on:` entry takes a persisted thread event, a domain event your workspace emits, or a persisted **system event**: `BackupCompleted`, `BackupFailed`, `NotificationCreated`, `TriggerCompleted`, `PluginInstalled` and the rest (ADR 0113). "Tell me if a backup fails" needs no new emitter. What stays out is a transient frame such as `BackupProgress` or `Toast`, which writes no row and reaches no matcher. Subscribe to the event that ends the run instead. `system-knowhow/thread-events/one-table-two-enums.md` § "Today the scheduler uses a blocklist" carries the full rule.

**A persisted event fires its trigger even under heavy load.** When a burst of events outruns the scheduler, it reads the missed ones back from the event log and fires for them, once each. A transient domain event (one emitted with `transient: true`) writes no row, so there is nothing to read back. A trigger on one is best-effort: under heavy load it can miss a fire. When every occurrence matters, emit the event durably.

**Do not guess the name.** Create and update both check every `on:` entry. A misspelled or retired engine name is refused, with the real one named, because an exact-string match would arm clean and never fire. A name outside the engine's set is accepted, with a warning when this workspace has never emitted it. Look one up with the `events` tool's `event_types` action.

### One trigger, multiple events

The `on` field is a list. Use multiple entries when *one workflow* reacts to several event types. Example: "summarize my day on `MessageReceived` from my partner OR on `EmailReceived` from my boss". Two parallel triggers with the same intent are a UX trap: edit one, forget the other, and behaviour silently drifts.

Each entry carries its own `condition`, scoped to *that* event:

```json
{
  "on": [
    { "event_type": "OuraSleepImported", "condition": { "sleep_score": { "$lt": 70 } } },
    { "event_type": "EmailReceived" }
  ]
}
```

The `sleep_score` filter does NOT apply to `EmailReceived`, whose payload has no such field. Per-entry conditions keep different payload shapes from constraining each other.

### Aggregating events: cron, per event, or a projection

"If an event exists, prefer it" covers how you learn something happened. It does not settle how to maintain an **aggregation**: a rollup, running total, per-period summary or counter, any current state derived from N events. Both shapes are legitimate:

- An **event subscription** buys freshness, and costs one run per event.
- A **cron** buys a bounded, predictable cost, and costs staleness of up to one interval.

The consumer decides which matters more: what it does with the number, and how wrong the number may be between updates. What follows helps you answer that; it is not a verdict.

**Often the answer is a projection rather than either trigger shape.** For a busy event type, fold each event into a maintained read model and point consumers at it. That beats every consumer re-aggregating raw events. The projection is the artifact: a stored value plus a cursor recording how far it has consumed. What advances it (a cron recompute, an O(1) per-event increment, a reader that merges the tail itself) is a smaller, separate question.

Think twice about a trigger per event on a busy event class, because **a trigger fire is a thread, not a callback**. Each one goes through thread-queue admission and spawns a real process. That fixed cost does not shrink as fires get denser.

Measure two things before picking:

- **Does the per-fire work shrink as fires get denser?** Split the measured per-run cost into three parts: fixed overhead (process start, connection), work proportional to NEW rows, and work proportional to the WINDOW recomputed. Only the middle part shrinks. If the window term dominates, firing more often multiplies a near-constant, and only measurement shows it.
- **Can you keep up at the PEAK rate, not the average?** Multiply peak rate by per-fire cost. Above one unit of work per unit of wall clock, the fires cannot drain. Event fires never coalesce the way cron fires do: with `max_concurrent_per_trigger` at 1 they queue strictly FIFO, the backlog runs into `max_queued_per_trigger` (25), and `overflow` drops the oldest waiting fires. A projection that breaks at the peak stays broken. See `system-knowhow/thread-queue.md`.

One point is not about cost: **a whole-window recompute is idempotent and self-healing.** Rerun it and nothing changes; miss a run and the next one repairs the gap. An incremental per-event append is neither: a rerun double-counts, and a missed fire is silent drift. That is a reason to lean toward recompute, or to pair an incremental path with a reconciling recompute. It does not rule incremental out.

**One thing you do not have to design around: a trigger is never woken by an event its own fire emitted.** Every event a fire emits carries the emitting trigger's id, and the matcher drops that trigger from the matches. Other subscribers still see the event.

So subscribing to an event class your own run emits needs no defensive narrowing. An *intent* trigger may subscribe to `ResponseGenerated`, which its own model call emits. Broad-subscribe plus a cheap internal gate is a supported shape, such as an idle detector watching every terminator event, `TriggerCompleted` included.

The suppression covers your fire and stops where the fire hands work off. Your script counts as the fire, so what it emits is marked too. That covers a bash or python tool, and any emit through the `lucidos` CLI or from Python: both attach the fire's signed token for you. Bare `curl` does not, so its emit wakes you like anyone else's: use the CLI. A sub-thread the fire spawns, or a coding-agent session it starts, is a handoff. It emits unmarked and still wakes you, on purpose: a trigger waiting on a session it started must fire when that session reports back.

An app emitting through the SDK is nobody's fire, so its events wake every subscriber.

`max_event_trigger_depth` (a *capacity policy* field, default 5) still bounds a chain ACROSS triggers, where A's fire wakes B and B's fire wakes A. Past the cap an event is still stored and still reaches SSE, but fires no further triggers. Unlike the marker, the depth DOES follow work a fire hands off, so a spawn buys no fresh chain. Two consequences:

- **A trigger waiting on a coding agent it started still fires.** Depth counts trigger fires, not tasks, so hop 1 is nowhere near the ceiling.
- **A long legitimate chain can hit the ceiling.** If it does you get a notification naming the trigger that did not fire, so it never stops silently. Raise `max_event_trigger_depth` in the Thread Queue panel if the chain is meant to be that long.

Shapes, roughly in the order they tend to fit:

1. **A cron for the projection, with the consumer merging the recent tail itself.** It queries the event store for rows above the projection's stored cursor and folds them on top. So the reader is current without the projection being current. This is what the Token Cost dashboard does.
2. **O(1) per-event work with no database round trip**, when the projection has to be current between runs. Read the row out of `TRIGGER_EVENT_PAYLOAD`, which the engine already hands the script, increment, write. A cron underneath as a reconciling rebuild buys back the self-healing above.
3. **The plain event trigger**, when the event is low-volume or user-initiated. The hybrid shape is this one plus a floor: a cron, plus `on: SomethingRequested` for a Refresh button.

Read a rate as a smell, not a threshold. An event firing more than about once a minute, sustained, is worth measuring before you subscribe a trigger to it. The number settles nothing alone; it is where the two questions above start to give different answers. `system-knowhow/thread-events/wire-format-and-metadata.md` § "Volume classes" labels each engine event. `lucidos events count` gives a workspace's actual rate.

**Worked example** of that measurement, not the source of any threshold (a live workspace, August 2026). `ContextCaptured`, one row per model API call, ran 7,500 to 17,600 rows a day. It peaked at 2,415 in one hour and 238 in a single minute. Its rollup script took 3.4 seconds per run. Per event, that is 7 to 17 hours of query time a day, against about 82 seconds a day as an hourly cron (24 runs). The peak minute alone would need just over 13 minutes of work (238 runs) for 60 seconds of events.

Process start (29 ms) and psql connect (25 ms) were negligible: the 3.4 seconds sat in one SQL query. Its window function scans each touched thread's full history, which does not get cheaper when you fire more often.

