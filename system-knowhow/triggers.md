---
name: Triggers
description: Use when the user wants something to happen automatically ("every morning", "notify me when X happens", "watch for Y") or an EXISTING trigger to run right now ("run this trigger now", "fire it manually"). Load it even if a trigger may not be the answer: it settles that, and routes "tell me HERE when X happens" to `await_event` instead.
---

# Triggers

The working reference for *triggers*: choosing one, building it, editing it, and running an existing one off-schedule. The engine system prompt and the grouped `triggers` tool description carry the cron format and field reference, so this file does not restate them. `docs/taxonomy.md` § Triggers has the intent-versus-procedure worked example.

> **Tool surface.** The grouped **`triggers`** tool (`action: create | list |
> update | delete | pause | resume | run`) and the grouped **`trigger_groups`**
> tool (`action: list | create | rename | reorder | delete`) manage triggers.
> Here a bare verb such as `update_trigger` or `list_trigger_groups` is short for
> that tool with the matching `action`: `update_trigger(trigger_id, …)` means
> `triggers(action="update", trigger_id, …)`. The old flat names still work as
> aliases, but the model sees the grouped tools. The CLI mirrors them as
> `lucidos triggers …` / `lucidos trigger-groups …` (see `lucidos-cli.md`).

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

If the user says "notify me when X" and X isn't an event yet, you have two work items: (1) make X emit an event, (2) trigger on it. Tell the user that explicitly.

**Check first whether the engine already emits it.** An `on:` entry takes a persisted thread event, a domain event your workspace emits, or a persisted **system event**: `BackupCompleted`, `BackupFailed`, `NotificationCreated`, `TriggerCompleted`, `PluginInstalled` and the rest (ADR 0113). "Tell me if a backup fails" needs no new emitter. What stays out is a transient frame such as `BackupProgress` or `Toast`, which writes no row and reaches no matcher. Subscribe to the event that ends the run instead. `system-knowhow/thread-events.md` § "Today the scheduler uses a blocklist" carries the full rule.

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

1. **A cron for the projection, with the consumer merging the recent tail itself.** It queries the event store for rows above the projection's stored cursor and folds them on top, so the reader is current without the projection being current. This is what the Token Cost dashboard does.
2. **O(1) per-event work with no database round trip**, when the projection has to be current between runs. Read the row out of `TRIGGER_EVENT_PAYLOAD`, which the engine already hands the script, increment, write. A cron underneath as a reconciling rebuild buys back the self-healing above.
3. **The plain event trigger**, when the event is low-volume or user-initiated. The hybrid shape is this one plus a floor: a cron, plus `on: SomethingRequested` for a Refresh button.

Read a rate as a smell, not a threshold. An event firing more than about once a minute, sustained, is worth measuring before you subscribe a trigger to it. The number settles nothing alone; it is where the two questions above start to give different answers. `system-knowhow/thread-events.md` § "Volume classes" labels each engine event, and `lucidos events count` gives a workspace's actual rate.

**Worked example** of that measurement, not the source of any threshold (a live workspace, August 2026). `ContextCaptured`, one row per model API call, ran 7,500 to 17,600 rows a day. It peaked at 2,415 in one hour and 238 in a single minute. Its rollup script took 3.4 seconds per run. Per event, that is 7 to 17 hours of query time a day, against about 82 seconds a day as an hourly cron (24 runs). The peak minute alone would need just over 13 minutes of work (238 runs) for 60 seconds of events.

Process start (29 ms) and psql connect (25 ms) were negligible: the 3.4 seconds sat in one SQL query. Its window function scans each touched thread's full history, which does not get cheaper when you fire more often.

## Writing cron expressions

Six fields, `second minute hour day-of-month month day-of-week`, in the user's local timezone. Two rules decide what a trigger actually fires on, and they pull in opposite directions:

- **Within one expression, the fields are ANDed.** Every field must match. So when day-of-month AND day-of-week are both set, the expression fires only on days that satisfy both. (Vixie cron ORs those two specific fields. Lucidos does not. Never write a cron on the Vixie assumption.)
- **Across the array, the expressions are ORed.** A trigger's `cron` takes a list, and it fires at the earliest match from any entry. This is how you express "either of these".

The recipes below rely on both. Neither is a bug, and neither will change.

### The footgun: one expression is not "either"

`0 0 9 1 * Mon` reads to almost everyone as "the 1st, plus every Monday". It means "the 1st, but only when the 1st IS a Monday". That happens about 1.7 times a year on average and in lumpy gaps: it fires 2026-06-01, then nothing until 2027-02-01, then 2027-03-01, then nothing until 2027-11-01.

"The 1st, plus every Monday" is two expressions:

```json
{ "cron": ["0 0 9 1 * *", "0 0 9 * * Mon"] }
```

The engine warns (without refusing) when a single expression restricts both fields in a shape that fires rarely. It stays deliberately quiet for the 7-day windows below, which use the same AND on purpose.

### Day-of-week numbering

Write day-of-week in standard cron numbering: 0 and 7 both mean Sunday, 1 is
Monday, and 6 is Saturday. The engine translates it before parsing, so you never
write the crate's own numbering.

The `cron` crate numbers days 1 (Sunday) through 7 (Saturday).
`translate_dow_for_cron_crate` in
`crates/lucidos-engine/src/engine/tools/scheduler.rs` rewrites each plain
numeric day, or plain `a-b` range, as `(n % 7) + 1`.

| Day | Write (standard) | `cron` crate sees |
|---|---|---|
| Sunday | 0 or 7 | 1 |
| Monday | 1 | 2 |
| Tuesday | 2 | 3 |
| Wednesday | 3 | 4 |
| Thursday | 4 | 5 |
| Friday | 5 | 6 |
| Saturday | 6 | 7 |

Named days (`Mon`, `MON-FRI`, `SAT,SUN`) skip translation. The crate numbers
names Sunday-first too, so a plain named day or named range is safe as written.
The check runs per comma segment, so a mixed field (`Mon,1`) still shifts the
numeric part: `1` becomes Monday, same as `Mon`.

Three edge cases, each an error rather than a silent wrong day:

- An out-of-range numeric token (`8`, `999`) stays untranslated, and the crate
  rejects it.
- A numeric range-and-step token (`1-5/2`) fails validation, even inside a mixed
  field like `Mon,1-5/2`, since the engine cannot shift it safely. Write the days
  out (`1,3,5`), or use the named form (`Mon-Fri/2`), which fires on the days
  written.
- A range that crosses Sunday fails in both forms. `5-0` (Friday through Sunday)
  becomes `6-1`, and `6-1` (Saturday through Monday) becomes `7-2`. The crate
  rejects a start above the end. `Fri-Sun` numbers to `6-1` internally and fails
  the same check. List the days instead (`5,6,0` or `Fri,Sat,Sun`), or split
  into two cron expressions.

### nth weekday of the month

The AND is the mechanism here, not a trap: pin day-of-week and give day-of-month a **7-day window**. Any 7 consecutive dates contain each weekday exactly once, so this fires exactly once a month.

| Want | Cron |
|---|---|
| First Monday, 09:00 | `0 0 9 1-7 * Mon` |
| Second Tuesday, 09:00 | `0 0 9 8-14 * Tue` |
| Third Friday, 09:00 | `0 0 9 15-21 * Fri` |

All three verified exact for every month from 2026 to 2100.

### Last weekday of the month

Same trick from the other end, except the window has to move with the month's length, so it takes three ORed expressions:

```json
{
  "cron": [
    "0 0 9 25-31 1,3,5,7,8,10,12 Mon",
    "0 0 9 24-30 4,6,9,11 Mon",
    "0 0 9 22-28 2 Mon"
  ]
}
```

Verified against every month from 2026 to 2100 (900 months): exact in 898, with zero double-fires. The two misses are **February 2044 and February 2072**. They are the only leap years in that range where Feb 29 falls on a Monday, and there it fires Feb 22 instead. A fourth expression, `0 0 9 23-29 2 Mon`, fixes those two months but makes them fire **twice** (the 22nd and the 29th). So three is the better trade. Tell the user about this edge when you build one.

Swapping the weekday gives last Friday, last Tuesday and so on, with the same shape and its own leap-February exception (for Friday it is 2036, 2064 and 2092). For plain **month end** with no weekday, pin the last day per month-length class instead: `["0 0 9 31 1,3,5,7,8,10,12 *", "0 0 9 30 4,6,9,11 *", "0 0 9 28 2 *"]`. February is the awkward one either way, since its last day moves; `28` is a day early in leap years, and `28,29` fires twice in them. Pick one with the user.

### Expressions that can never fire

A day-of-month that the month is never long enough to contain is valid syntax and a dead schedule. These all parse cleanly:

| Expression | Why it never fires |
|---|---|
| `0 0 9 31 2 *` | February has no 31st |
| `0 0 9 30 2 *` | February has no 30th |
| `0 0 9 31 4,6,9,11 *` | April, June, September and November have 30 days |
| `0 0 9 30 2 Sun` | impossible date; the weekday is irrelevant |

**The engine rejects all of these at create and update**, naming the offending fields (`day-of-month 31 never occurs in month 2 (February)`). Fix the expression; there is nothing to work around.

`0 0 9 29 2 *` is NOT in this class. Feb 29 is rare, not impossible: it fires 2028, 2032, then 2036. Every create and update reports the **next 3 fire times** back to you, and the trigger's row in the panel shows them too. Read them against what the user asked for before you confirm: three dates a year apart when they said "monthly" is the tell.

## `condition`: when to filter

Set `condition` on a trigger subscription when the event is high-volume and you care about only a slice. Example: subscribe to `EmailReceived` but fire only on emails from one sender. Without a condition, the trigger fires for every email and the run has to filter, which is slow and wasteful.

Conditions are pure payload filters, so never use one for logic that depends on external state ("only if this app's data file says X"). Stateful checks belong inside the run.

**One field is always available on a thread event: `thread_id`.** It is not in any event's payload: the engine supplies it from the thread the event belongs to. It scopes a subscription to a single thread, so `{ "event_type": "CodingAgentIdled", "condition": { "thread_id": "<uuid>" } }` fires only when THAT coding-agent session reaches a turn boundary. A **domain event** (one your workspace emits with `emit_event`) belongs to no thread. It has no such field, so a `thread_id` condition on one matches nothing. Everything else a condition names is a **field path** into that event's own payload.

A persisted **system event** is the same case. `BackupFailed` belongs to no thread, so a `thread_id` condition never matches it. Condition on the variant's own fields instead, such as `filename` on `BackupCompleted`. The stored row wraps the event in a `type` / `data` envelope, and the matcher unwraps it for you, so never name those two keys.

**`ChangeProposed` also records parked work.** It fires for a new or updated change, including one the engine files straight into set-aside (`set_aside: true`). That covers work found on an archived thread's branch, and the first boot on a new version can file many at once. A trigger for "a change is ready to apply" therefore carries `{ "set_aside": { "$ne": true } }`. A pending proposal omits the field, so a bare `false` matches nothing.

### Who emitted a domain event: `actor`

**A domain event's `actor` is written by the engine, never by the emitter.** The engine records who called, and it drops any `actor` the emitter put in its own payload. So a condition on `actor.kind` reads who really emitted the event:

| Emitted by | `actor.kind` | Also carries |
|---|---|---|
| An app, through `lucidos.events.emit` | `device` | `device_id` only, never a name (see `thread-events.md`) |
| The `emit_event` tool (the Lucidos Agent) | `agent` | `agent.kind` = `lucidos_agent` |
| `lucidos events emit` from a coding agent, a script trigger or a `run_bash` script | `api` | `mode` = `agent`, and `source_thread_id` when a thread ran it |
| A webhook delivery | `webhook` | `webhook_id`, `name` |

So `{ "actor.kind": "device" }` fires only on an emit from a device, and no agent can satisfy it by writing an `actor` field. That makes it the right condition for a button that must be pressed by the user.

It is attribution, not proof of a tap. A device id is a header, and the engine does not authenticate it. So pair the condition with the payload fields the button itself sends.

### What a condition can say

A key is a **field path**. A bare name reads a top-level field, and dots read downwards: `{ "payload.workflow_run.event": "schedule" }` reads `event` inside the `workflow_run` object of a GitHub delivery. The leading `payload.` is not decoration; see § "The envelope" below.

Two rules keep a path honest. A key that exists verbatim wins at every level, so a webhook field literally named `a.b` is still nameable, even nested under another key. A path that resolves to nothing is null, exactly like a missing top-level field, so `{ "x": { "$ne": null } }` reads as "x exists and is not null".

A numeric segment is an ordinary object key, never an array index. There is no way to say "any element of this array matches", so filter arrays inside the run.

**A path you guessed is reported when you write it.** Every field path is checked against the twenty most recent stored payloads of that event type. A path in none of them gets a warning naming the real one. It is a warning rather than a refusal, because an optional field is legitimately absent from a sample. An event type this workspace has never emitted says nothing, having nothing to check against.

Operators, every one of which reads a field path:

| Operator | Matches when |
|---|---|
| bare value | the value is exactly equal |
| `$eq` / `$ne` | equal / not equal |
| `$lt` `$lte` `$gt` `$gte` | numeric comparison |
| `$in` / `$nin` | the value is in / is not in the list |
| `$regex` | the value is a string containing a match |

`$regex` is an unanchored search, so `^` and `$` anchor it and `(?i)` makes it case-insensitive. It only ever matches a JSON string: a number or an absent path is a miss.

**AND is implicit.** Several keys in one condition all have to hold, and several operators on one key do too: `{ "tokens": { "$gte": 1000, "$lt": 5000 } }` is a range.

**OR has two shapes.** `$in` ORs over one field's values. `$or` takes a list of whole conditions and ANDs with its siblings:

```json
{
  "payload.action": "completed",
  "$or": [
    { "payload.workflow_run.conclusion": "failure" },
    { "payload.workflow_run.conclusion": "timed_out" }
  ]
}
```

A bad condition is refused when you create or update the trigger, naming what is wrong. An unknown operator, an unparseable `$regex` and a malformed `$or` are all errors rather than a trigger that arms and never fires.

### The envelope: a webhook's body lands under `payload`

A webhook does not store what the sender posted. It stores a three-key envelope, so a GitHub `workflow_run` delivery arrives like this:

```json
{
  "summary": "github webhook fired",
  "headers": { "X-GitHub-Event": "workflow_run" },
  "payload": { "action": "completed", "workflow_run": { "conclusion": "failure" } }
}
```

The sender's entire body sits under `payload`, and the request headers you allow-listed sit under `headers`. So GitHub's own `action` field is `payload.action`, and its `workflow_run` object is `payload.workflow_run`.

| Condition | What it does |
|---|---|
| `{ "action": "completed" }` | matches nothing, ever |
| `{ "payload.action": "completed" }` | correct |

`delivery_payload` in `crates/lucidos-engine/src/api/webhooks.rs` builds the envelope. Two tests pin it: `a_senders_own_fields_land_under_payload` beside it, and `a_delivery_becomes_summary_headers_and_payload` in `crates/lucidos-e2e/tests/api_support/webhook_delivery_test.rs`, which reads the stored row back.

**Nothing warns you, and that makes this expensive.** A missing path and a present-but-null field both read as null, and the matcher cannot tell them apart. A subscription that can never match looks exactly like one patiently waiting. It arms clean, its panel row stays healthy, and `last_run` keeps the last real fire's timestamp.

**So diagnose by comparison, not by reading the panel.** Query the event store for the event type you subscribed to, and hold its newest row against the trigger's `last_run`. Deliveries arriving with no runs beside them is the tell.

**The general rule: write the condition against the STORED event, not the upstream payload you think you are subscribing to.** Only the shape of the row in the event store decides what a field path resolves to. So read one real stored event first, with the `events` tool's `query` action and `limit` 1, then write every path from what you see. A path copied out of the sender's API docs is a guess.

**Script triggers inherit the same envelope.** `TRIGGER_EVENT_PAYLOAD` holds the whole event payload, wrapper included, so a script has to reach through `payload` too. A hand-emitted test event is usually written flat, so a script can pass your test and still fail on the real delivery. Read it defensively:

```python
raw = json.loads(os.environ.get("TRIGGER_EVENT_PAYLOAD", "{}"))
body = raw.get("payload") if isinstance(raw.get("payload"), dict) else raw
```

## Notification discipline

Call `send_notification` only when there is something the user wants to hear about. A morning summary that finds nothing new sends no notification: silent success is the norm.

The scheduler auto-creates an error notification when a trigger fails, so don't double-notify on errors from inside the run.

## Where the thread lands: `go_to_review`

By default, trigger runs are unattended: their threads go straight to Archive when they finish. They surface in the Current section only if the user follows up with a message. That suits most cron triggers (silent imports, periodic syncs, idle nudges).

A run that stops to ask the user a question, or to ask permission, always surfaces in Current, whatever `go_to_review` says. It stays there until the user answers or stops it.

Set `go_to_review: true` when the trigger's *output is the point*: a daily summary the user means to read, an alert that needs acknowledgement, a scheduled report. The thread then surfaces in Current on completion instead of getting lost in Archive.

In the trigger form this is the **Send directly to Archive** toggle, which reads the field inverted: on (the default) is `go_to_review: false`, and off is `go_to_review: true`. Name the toggle that way when you point the user at it.

| User phrasing that answers it | Flag |
|---|---|
| "import my data", "sync X", "keep Y up to date" (silent housekeeping) | omit (default false) |
| "put it in front of me", "make sure I see it", "I want to read this" | `go_to_review: true` |
| "summarize my week", "write a report I should look at" (output is the point) | `go_to_review: true` |

A `send_notification` does **not** answer this question: notifications and the review surface are independent. A "notify me when X" trigger may or may not also need its thread in review, and the user has to say which.

If the request doesn't clearly land in one of the rows above, **ask** (Question 5 below). Each run snapshots the flag when it fires, so toggling it later affects only future runs.

## Which model the run uses: `model` and `reasoning_effort`

An intent trigger fires on the account chat defaults (Settings → Models →
Chat & triggers) unless it says otherwise. Set `model` to pin it to a specific
chat model and `reasoning_effort` to pin its thinking budget
(`none|low|medium|high|xhigh|max`); omit either, or send null, to go back to the
account default. The two are independent: pinning the model leaves the effort on
the account setting, and the reverse.

Script triggers have no model. They run no LLM, so these fields and `provider`
below are ignored there, and the form hides them.

| User phrasing | What to set |
|---|---|
| "use something cheap for this", "it's just a digest" | a low-cost model, often with a low `reasoning_effort` |
| "this one needs to be thorough", "use the best model" | the stronger model, and usually a higher `reasoning_effort` |
| nothing about models | omit both, so the trigger follows the account default |

Pin a model only when the user asked for one. A pinned trigger stops following
the account default, so a workspace-wide model change no longer reaches it.

Like the `chat_model` preference, the model id is **not checked against the
registry at save**, since a model can be disabled or deleted later. A wrong id
fails at fire time, as a normal trigger-failure notification. Use
`manage_models(action='list')` to see the real ids.

The model and effort a run actually used are recorded on its `TriggerStarted`
event, so the trigger's thread shows what it ran on and a follow-up there
continues on the same model rather than snapping to the account default.

### Which backend serves the model: `provider`

A model can have more than one route, one per backend that serves it. With no
`provider`, the run follows the model's own *preferred provider*, then its first
configured route. Set `provider` only when the user asks for a specific backend
("run this one on the direct Anthropic API").

Unlike the model id, a `provider` pin **is checked when you save**:

- it must be a provider name (`manage_models(action='list')` shows each model's
  routes);
- it needs a `model` pin, since it says which backend serves that model;
- it must be one of that model's routes.

Null clears the pin. Changing the trigger's `model` without restating
`provider` clears it too, because a pin belongs to the model it was picked for.

A pin to a backend with no credential **refuses the fire** rather than running
it on another backend. A trigger fires unattended, so a silent move to another
vendor is a change nobody would notice.

### Notification routing (`app_id`, `tap`, `event_id`)

Three independent fields control the notification:

- **`app_id`**: *which* app the notification is about. It drives the inbox modal's "Open <app>" button. Set it whenever the notification relates to a specific app, even when the tap routing is `{ kind: 'modal' }`.
- **`tap`**: *what happens on tap*. A discriminated union of `{ kind: 'modal' }` and `{ kind: 'navigate', to: NavigateUi }`. The default, `modal`, opens the inbox detail showing the body, and suits informational pushes too, since every notification is openable. `navigate` delegates to the same router `navigate_ui` uses, and `to` is its arg shape. Both mark the source notification read on tap. The passive `{ kind: 'none' }` was retired (`docs/plans/2026-07-02-remove-notification-tap-none.md`).
- **`event_id`**: *which event inside the linked thread* raised the notification. Optional UUID. The §4 in-app matrix uses it to mark the notification read silently when the user is already looking at the source event. It is distinct from `tap.to.event_id`, the scroll-and-pulse target when the tap opens a thread, though the two usually hold the same value.

Write the **`message` as content only, and never restate the `title` in it.** The in-app toast, the inbox detail and the OS push each show the title in their own right. So a body that opens by repeating the title shows it twice. Use a bare sentence for a single item and `"• "`-prefixed lines for a list. The toast shows each line as written, under the bold title. See `system-knowhow/notifications.md` §4.

| Trigger says | `app_id` | `tap` | `event_id` |
|---|---|---|---|
| 8:00 habit-tracker "Check in for today" (direct CTA inside an app) | habit-tracker | `{ kind: 'navigate', to: { target: 'app', app_id: 'habit-tracker' } }` | omit |
| Coding agent is asking the user a question (needs them back in the conversation, on that question) | omit | `{ kind: 'navigate', to: { target: 'thread', id: '<thread_id>', event_id: '<event_id>' } }` | source event id |
| Coding agent is asking for permission (same idea, different event) | omit | `{ kind: 'navigate', to: { target: 'thread', id: '<thread_id>', event_id: '<event_id>' } }` | source event id |
| "5 changes ready to apply" (multi-item panel destination) | omit | `{ kind: 'navigate', to: { target: 'changes' } }` | omit |
| Daily summary "you completed 5 tasks today" (informational, no CTA) | omit | `{ kind: 'modal' }` (default) | omit |
| 22:00 bedtime nudge (informational) | omit | `{ kind: 'modal' }` (default) | omit |
| Habit-tracker weekly report (about an app, but the action is reading) | habit-tracker | `{ kind: 'modal' }` (default) | omit |
| "Backup complete" / "Sync finished" (purely informational, no action needed) | omit | `{ kind: 'modal' }` (default) | omit |

Tap defaults to `{ kind: 'modal' }` so the user reads the message and decides. `navigate` is the explicit opt-in for direct CTAs and panel deep-links. The notification always lands in the inbox, whatever `tap` says, so the user can reopen it from the bell icon.

See `system-knowhow/js-sdk.md` § `lucidos.notifications` for the full `NavigateUi` target list (panels, apps, threads, files, triggers, creation forms, URLs).

#### Where the LLM finds `event_id`

When a trigger fires from a `BusEvent::Thread` match, the engine appends a `## Triggering Event` block to the trigger's user message. Above the JSON payload, a line like:

```
Source event id: 7a9c2c5f-…
```

…carries the UUID of the event that fired the trigger. Pass it as `send_notification`'s `event_id`. The push tap then deep-links to that exact event, and the question card pulses on land.

**Only a thread event qualifies.** A workspace domain event (anything `emit_event` wrote, such as `E2ETestsPassed`) gets a `Source event id` line too, but it lives in no thread. No transcript can show it, so `send_notification` refuses it as `event_id`, and `lucidos notify` refuses it as `--event-id`. Omit `event_id` for those, and the tap opens the notification card. The same refusal covers an event from a different thread than the one the tap opens.

For schedule (cron) triggers there is no source event, so no `event_id`. For on-event triggers that notify about *a different* event (e.g. fire on `CodingAgentIdled` but notify about the last `UserQuestionAsked`), look the right event up yourself with `query_events` and use that id.

#### Worked example: push when agent needs me

```yaml
on:
  - event_type: UserQuestionAsked
run:
  intent: "Notify me when the agent has a question waiting for me. The push should deep-link straight to the question: tapping it takes me to the originating thread and pulses the question card on land."
```

The same shape works for `event_type: CodingAgentPermissionRequest` (read the message from the `tool_name`/`summary` fields). Lucidos does not seed this trigger: workspaces opt in by creating it.

## Script triggers: when an LLM call is overkill

A trigger's `run` takes one of two shapes. `{ "type": "intent", "intent": "…" }` is the LLM path above. `{ "type": "script", "path": "triggers/<slug>/scripts/run.py" }` runs a script directly, with no LLM. Pick `script` when the work is mechanical and LLM judgement is not the feature. Examples: a fixed shape applied to whatever events `on:` selects, a scripted API call, a deterministic emit.

Good candidates for `script`:

- "On any event in `on:`, notify with title + message read from the payload's common fields."
- "Every morning at 7, hit `<API>` and write the response to `data/artifacts/<date>/x.json`."
- "On `OrderPlaced`, emit `OrderQueuedForShipping` if `order.total > 100`."

Bad candidates for `script` (keep these as `intent`):

- Anything that needs to read the workspace's intent registry / knowhow library to pick a procedure.
- Anything where the message wording should adapt to context (the LLM's judgement is the feature).
- Multi-step workflows whose branches depend on prior results: the LLM as coordinator is what makes them work.

### Scripts run in place: `__file__` is the real path

The engine executes a registered script **from its real location on disk**, with the workspace root as the working directory. So `__file__` is `<workspace>/data/triggers/<slug>/scripts/run.py`, and the ordinary way of reaching a sibling directory works:

```python
_STATE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "state")
```

That resolves to the real `data/triggers/<slug>/state/`, the natural home for a script trigger's own state (a last-seen id, a per-version marker, a cursor). Prefer `__file__`-relative paths over paths relative to the working directory. They keep the script correct whoever invokes it, and keep its state beside the script that owns it (the ownership rule in `docs/taxonomy.md`).

A trigger that a plugin ships is the exception. Its folder belongs to the plugin, so its state goes under `data/artifacts/<plugin-id>/` instead. See `plugins.md` § "Where a plugin keeps its runtime state".

### Script trigger env vars

When the engine fires a script trigger that subscribes to a domain event, it sets these env vars before it runs the script. A schedule fire sets none of them, since there is no source event.

| Env var | Set when | What it holds |
|---|---|---|
| `TRIGGER_EVENT_TYPE` | Always on event fires | The matched event name (e.g. `UserQuestionAsked`). Use as a fallback title or when the script genuinely needs to branch on type. |
| `TRIGGER_EVENT_PAYLOAD` | Always on event fires | The source event's payload, serialized as JSON. Parse with `json.loads(os.environ["TRIGGER_EVENT_PAYLOAD"])`. |
| `TRIGGER_EVENT_ID` | When the source event has a row id | The `events.id` (UUID) of the source row. Pass it to `lucidos notify --event-id`, beside `--thread-id "$TRIGGER_EVENT_THREAD_ID"`, so the push tap scroll-and-pulses the exact card. A domain event has no thread, so leave it out then: the engine refuses an event outside the linked thread. |
| `TRIGGER_EVENT_THREAD_ID` | Only for *thread-scoped* source events | The thread the source event lives on. Pass to `lucidos notify --tap navigate --thread-id` so the push deep-links to the originating conversation instead of the trigger's own thread (which is `LUCIDOS_THREAD_ID`). |

The trigger's own thread is `LUCIDOS_THREAD_ID`, the same env var every spawned subprocess gets. `TRIGGER_EVENT_THREAD_ID` is the *source* event's thread, a different one. Mix them up and the push deep-links into the trigger's own thread instead of where the user needs to act.

### Worked example: push when any subscribed event fires

The script is *event-agnostic*: the trigger's `on:` list decides which events fire it, so the same script keeps working as you add or remove events.

`data/triggers/when-agent-needs-me/scripts/run.py`:

```python
#!/usr/bin/env python3
"""Push a deep-linking notification for any event the trigger subscribes to.

The trigger's `on:` list decides which events fire this; the script
treats them uniformly. Title and message come from the payload's
common fields (`title`, `message`, `summary`, `question`); the event
type is only the fallback title. `--tap navigate` + the source
event's thread id + event id make the push land on the exact card the user needs to act on.
"""
import json
import os
import subprocess

event_type = os.environ["TRIGGER_EVENT_TYPE"]
payload = json.loads(os.environ.get("TRIGGER_EVENT_PAYLOAD", "{}"))
thread_id = os.environ.get("TRIGGER_EVENT_THREAD_ID")
event_id = os.environ.get("TRIGGER_EVENT_ID")

title = payload.get("title") or event_type
message = (
    payload.get("message")
    or payload.get("question")
    or payload.get("summary")
    or f"{event_type} needs your attention"
)

args = ["lucidos", "notify", "--title", title, "--message", message]
if thread_id:
    args += ["--tap", "navigate", "--thread-id", thread_id]
    if event_id:
        args += ["--event-id", event_id]

subprocess.run(args, check=True)
```

The trigger config picks the events:

```json
{
  "name": "When agent needs me",
  "on": [
    { "event_type": "UserQuestionAsked" },
    { "event_type": "CodingAgentPermissionRequest" },
    { "event_type": "CredentialRequested" },
    { "event_type": "McpConsentRequested" }
  ],
  "run": {
    "type": "script",
    "path": "triggers/when-agent-needs-me/scripts/run.py"
  }
}
```

Want to also notify on `EmailReceived` from your boss? Append another `on:` entry; the script doesn't change. `run.path` is workspace-relative, and the engine resolves it under `data/`. Swapping `intent` for `script` drops one LLM call per fire, with no change the user can see.

If a payload lacks the well-known fields, the push shows only the fallback title (the event type) and a generic message. That is the cost of the event-agnostic shape. Branching on `event_type` inside the script is a maintenance trap, since every newly subscribed event then needs a script edit. Better: carry `title` / `message` in the payload at the *event's* emit site, so any subscriber can render it.

## Grouping triggers

A *trigger group* is a user-visible folder, shown as a collapsible section in the triggers panel. Groups are pure labels: no schedule, no code, no coordinated firing. They only collect related triggers under one header so the panel stays readable.

Use a group when several triggers form one workflow and the user benefits from seeing them together. For example, one emits an event via `emit_event` and another listens via `on_event`. A single trigger needs no group; the "Ungrouped" section at the bottom of the panel holds it.

| Tool | When to use |
|---|---|
| `list_trigger_groups` | Before assigning a trigger, check whether a fitting group already exists. |
| `create_trigger_group(name, order?)` | Create a new section header. Names are unique within the workspace (case-insensitive). |
| `create_trigger` / `update_trigger` with `group_id` | Assign a trigger to (or move it between / out of) a group. `update_trigger(group_id: null)` clears membership. |
| `rename_trigger_group(group_id, name)` | Rename the section. |
| `reorder_trigger_groups([{id, order}, ...])` | Batch-reorder panel sections. |
| `delete_trigger_group(group_id)` | Refused if the group still has members; move or delete them first (the error response lists them). |

Groups are orthogonal to `app_id`: an app-owned trigger can live in any group. `app_id` drives notification deep-linking, `group_id` drives panel layout.

## Side-effect grant: authorizing unattended risk

This matters **only when the workspace has the command guard on** (Settings → Permissions → Command safety; off by default). The guard then classifies every `run_bash` / `run_python` command a trigger's intent runs. Most commands (reads, data crunching, downloads, writes inside the workspace) run untouched. An **irreversible** one is gated: sending email, a mutating HTTP request (POST/PUT/DELETE), a cloud-CLI change (`gh`/`aws`/`gcloud`), destroying files outside the workspace.

A chat turn would *ask* the user to approve such a command, but a trigger fires with nobody to ask. So the trigger carries a **side-effect grant**: the irreversible side-effect categories it is pre-authorized to perform. At fire time:

- the command's side-effect category **is in the grant** → it runs;
- it **isn't** → the command is blocked and **the whole trigger run fails** (a failure notification surfaces it, naming the blocked command and the missing grant, with an *Open trigger* button that lands on these settings).

The categories are **email**, **external API** (mutating HTTP), **cloud CLI** (gh/aws/gcloud), **out-of-workspace destruction**, and **other** (anything irreversible that fits none of those). The default grant is empty: a new trigger may perform *no* irreversible side-effect.

**The user sets the grant, not you.** The `create_trigger` / `update_trigger` tools accept no grant field, on purpose: an autonomous agent can't widen its own unattended authority. The user ticks the "Allowed side-effects" checkboxes in the trigger's settings. So when a trigger's intent needs an irreversible side-effect ("email me the digest every morning"), **tell the user** to tick the matching one. Here that is *Send email or messages*. With command safety on, the run otherwise fails the first time it tries to send; with it off, none of this applies.

**The grant also flows to coding-agent work the trigger spawns.** A *coding-agent thread* (Claude Code / Codex) that a trigger's intent launches runs **unattended**, directly or via a sub-thread an orchestrator spawns. Nobody answers its permission cards. So the engine walks the spawn tree to the root trigger and resolves each request from that trigger's grant:

- benign in-workspace work (reads, in-workspace edits, git, `lucidos data write` to `data/`) is auto-allowed;
- an irreversible side-effect is allowed only if its category is in the grant;
- a catastrophic command is always denied.

The chat command guard fails the *whole* run on an ungranted side-effect, but this path denies just the one request. The agent gets the denial and works around it or reports the step failed. This holds whatever the command safety toggle says. So a coding-agent trigger that needs a mutating HTTP call still needs **Call external APIs** ticked. Otherwise that one call is denied, and the rest of the run proceeds. See `coding-agent-events.md` § "Unattended auto-resolution".

The flow follows child spawns only. A top spawn, by `spawn_thread` with `relation: "top"` or by `lucidos spawn-thread` without `--relation child`, is independent of the trigger: its cards wait for a human. Spawn the coding agent as a child when the run must finish unattended. A spawn into another workspace is always a top spawn, so it always asks.

## Edit, don't recreate

**Always look for an existing trigger first** (`list_triggers`) and change it with `update_trigger`. Call `create_trigger` only when no comparable trigger exists. Recreating gives the new trigger a fresh `trigger_id` and orphans the old one's run history. The threads still exist, but no longer match the live trigger in the filter dropdown, trigger-scoped reports, or anything else that joins by id. The user sees no threads for a workflow that has fired for months.

This applies to every shape of change:

| User says | What to do |
|---|---|
| "Change the cron to 9am" | `update_trigger(trigger_id, cron=...)` |
| "Rename it to X" | `update_trigger(trigger_id, name="X")` |
| "Switch it to fire on event Y instead" | `update_trigger(trigger_id, cron=null, on=[{event_type:"Y"}])` |
| "Also fire when Z happens" | `update_trigger(trigger_id, on=[existing..., {event_type:"Z"}])`: append to the `on` array, don't make a sibling trigger |
| "Stop firing on event Y" | `update_trigger(trigger_id, on=[existing... minus Y])`: `on` is a full replacement |
| "Tighten the Y filter" | `update_trigger(trigger_id, on=[..., {event_type:"Y", condition:{...}}, ...])`: replace that entry inside the full list |
| "Tweak the prompt" | `update_trigger(trigger_id, run={...})` |
| "Pause it" | `pause_trigger(trigger_id)` (or `update_trigger(..., paused=true)`) |
| "Make sure I see this one" / "Send to review" | `update_trigger(trigger_id, go_to_review=true)` |
| "Stop bringing this up, keep it in the archive" | `update_trigger(trigger_id, go_to_review=false)` |
| "Add another time it should run" | `update_trigger(trigger_id, cron=[existing..., new_expr])`: append to the cron array, don't make a sibling trigger |
| "Run it once more, like at 7pm tonight" | `update_trigger(trigger_id, cron=[existing..., one_shot_expr])`, then another `update_trigger` after it fires to remove the one-shot entry. Never a duplicate trigger, not even temporarily |

If you truly need a different trigger (a different *workflow*, not a tweak), give it a clearly different name. Two live triggers with the same name are a UX trap: no picker tells them apart.

## Running an existing trigger once, off-schedule

**`triggers(action="run", trigger_id)`.** That is the whole answer for a cron trigger. The CLI is `lucidos triggers run --id <uuid>`, the SDK is `lucidos.triggers.run(id)`, and the trigger's row in the panel has a **Run once** button. (Not to be confused with the Thread Queue panel's **Run now**, which force-admits an entry that is *already queued* and cannot create a fire.)

**When you send the user to that button, link the TRIGGER, not the panel**: `[Nightly digest](trigger:<id>)`, with the id from `list_triggers`. The link lands on the trigger's own row, with **Run once**, the pause toggle and the last-run status. `[Triggers](triggers)` only opens the list.

It is a real fire. It records `TriggerExecuted` / `TriggerCompleted` and updates the panel's `last_run` and OK/failed status. It runs under the trigger's own identity and side-effect grant, and (for an `intent` run) its *trigger thread* and `go_to_review` routing. Downstream, nothing tells it apart from a scheduled fire. It returns once the run is admitted, not when the run finishes.

Three answers other than "started". Relay each as-is, never as a run:

- **Already running.** A fire of this trigger was already active or queued, so nothing new started: scheduled fires coalesce to at most one pending run per trigger. Tell the user that; do not claim you started one.
- **Paused.** Refused. Resuming runs nothing *on purpose*, so if the user wants both, do both. Re-registering the schedule does re-run the missed-slot catch-up, so a cron slot from the past hour that never ran fires on resume. That side effect is not a way to ask for a run, and you cannot predict it. A pause *you* just made counts at once: every trigger write is visible to the next call, so `pause_trigger` then `run` in one turn is refused, not raced.
- **No cron schedule.** Refused: an event-only trigger has never fired without a payload. An intent run would find no `## Triggering Event` block, and a script run would get none of the `TRIGGER_EVENT_*` vars. Emit its event instead.

### Event-only trigger: emit the event

`events(action="emit", …)`, or `lucidos events emit <Type> --summary "…" --payload '{…}'` from a script. The emit goes through the same matcher, admission and run as a real event, so it is the faithful reproduction.

- **Per-entry `condition` filters still apply.** A payload that fails the condition matches nothing and you get silence, not an error. Read the `on` array from `list_triggers` and build a payload that passes.
- **A condition on `actor` is not yours to pass.** The engine records you as the emitter, whatever the payload says (§ "Who emitted a domain event"). A trigger gated on `actor.kind` = `device` waits for the user. Ask them to press the button.
- Shape the payload like the real emitter's, not just enough to match: the run reads it (`## Triggering Event` for an intent, `TRIGGER_EVENT_PAYLOAD` for a script).
- The event is real and persisted, so every *other* subscriber fires too.
- **Event fires do NOT coalesce.** Each carries its own payload, so event fires keep strict FIFO, unlike the run action. Emit twice and the trigger runs twice, the second surfacing as an unexplained extra run minutes later. Check `list_threads` (rows carry `trigger_id` and `status`) before you re-emit.

### Don't imitate the fire

Copying `run.intent` into `run_thread`, or running the trigger's script yourself with `run_python` / `run_bash`, looks like a run and isn't one. It records no `TriggerExecuted`, no `last_run` and nothing in the trigger's history. It gets none of the trigger-fire framing, its system rules or its side-effect grant. Doing the work inline in the conversation is worse: there is no per-run thread to open, and a long or destructive procedure runs inside a chat turn.

Both are fine for **debugging** ("does the script still crash?"), if you call it that. A hand-run script gets none of `TRIGGER_EVENT_TYPE` / `TRIGGER_EVENT_PAYLOAD` / `TRIGGER_EVENT_ID` / `TRIGGER_EVENT_THREAD_ID`, so an event-driven one raises `KeyError` on the first lookup. Its `LUCIDOS_THREAD_ID` points at your conversation, so any `lucidos notify` lands in the wrong thread.

## What a trigger fire may not do to other triggers

**A trigger fire may act on itself and on nothing else, and even on itself it may not run itself.** The engine enforces it. Five scheduling tools are gated during a fire, and the `run` action is stricter still.

| Tool | On the firing trigger itself | On any other trigger |
|---|---|---|
| `create_trigger` | refused | refused |
| `update_trigger` | refused | refused |
| `delete_trigger` | allowed | refused |
| `pause_trigger` | allowed | refused |
| `resume_trigger` | allowed | refused |
| `run` | refused | refused |

Self-delete and self-pause are allowed because they **terminate**: the fire acts on its own id and stops. Self-run is refused because it **recurses**. With the per-trigger concurrency cap at 1, a self-run cannot start a second copy. Each fire instead adds one entry to a queue that never drains.

The consequence for whoever writes a trigger: **a trigger whose purpose is to re-arm, reschedule, pause, or reconfigure ANOTHER trigger cannot work.** Nothing validates this when you create it. It arms clean, its panel row looks healthy, and it fires on time. The work fails only at the moment it runs.

### What to do instead

The common case: a recurring trigger is paused so a one-off run can take its place, and something has to turn it back on. In order of preference:

1. **Do not pause the recurring trigger at all.** Have it read a skip marker, a small file or a domain event. It then skips its own next run when the marker is set. The recurring trigger stays armed throughout, so nothing has to re-arm it, and the skip clears itself.
2. **If it must be paused, the re-arm is a human action.** A one-shot trigger that NOTIFIES the user the paused trigger needs resuming does work, because a notification is not a scheduling call. Tell the user plainly that this is a reminder, not an automation.
3. **Do not route around the guard with `curl`.** The `run` refusal holds at the HTTP layer too, and the engine states the other guards explicitly. Posting to the engine API to reach another trigger is out of bounds.

## Questions to settle with the user before creating

Don't call `create_trigger` from the user's first message. Most "create a trigger for X" requests leave at least one of these unsettled, so confirm before you write the trigger. Skip a question only when the user already answered it in the same turn.

1. **Recurring or one-shot, and if one-shot, now or at a future time?** A recurring need is always a trigger. A one-off for **now** ("check X and tell me") is handled inline, with no trigger. A one-off at a **future time** ("remind me at 5pm today", "ping me in 20 minutes") needs a **one-shot trigger**. For any one-shot (even a test like "fire once in 2 min"), ask whether it should delete itself: it won't on its own. See "One-shot triggers" below for the procedure, `go_to_review` included.
2. **Cron or `on`?** "Every morning at 8" is cron. "When my package ships" is a trigger subscription. Several events for one workflow ("when X *or* Y happens") belong in one trigger with multiple `on` entries. If the event doesn't exist yet, name the work (emit the event from somewhere, then trigger on it) and confirm.
3. **What's the run.intent in the user's voice?** One sentence the user would actually say. If procedure comes to mind while you draft it, write the knowhow file first (§ "Write the knowhow file FIRST, then the intent").
4. **Should it notify, and on what?** Default is silent (§ "Notification discipline"). Confirm whether a successful run should notify, and what the message should look like.
5. **Surface to review or stay silent?** Always ask unless the user's phrasing clearly answers it (see the table in "Where the thread lands"). Set `go_to_review: true` for "I want to read this when it finishes"; omit it for silent housekeeping. A `send_notification` doesn't answer this question.
6. **If updating an existing trigger:** confirm which one (see "Edit, don't recreate" above).

Don't ask all six in one wall: pick the ones the request leaves open. "Every Monday at 9am summarize my open PRs and put it in front of me" answers cron, intent and review surface. Confirm only the notification shape, if it is unclear. "Say hello once in 2 minutes" answers cron and intent but **not** review surface, so confirm that before creating.

## One-shot triggers

A one-off that means "do this **now**" ("check X and tell me") needs no trigger: do it inline. A one-off anchored to a **future time** ("remind me at 5pm today", "ping me in 20 minutes") needs a one-shot trigger. Inline cannot work: you are not running then and nothing auto-resumes you, so an inline "reminder" is silently dropped. Also create one whenever the user explicitly asks (testing, a demo, deliberate scheduling). A one-shot is a normal trigger whose cron matches a single upcoming moment. It doesn't self-clean (below), so the self-deleting variant is usually what you want.

**Leave `go_to_review` at its default (false / omitted)**, so the single fire-thread goes straight to Archive. Its job is done the moment it fires, and the user doesn't need to read its thread. That holds **even when it sends a `send_notification` or deletes itself**: the notification is the user-facing output. Set `go_to_review: true` only if the user explicitly wants to read the run afterwards.

A one-shot trigger does **not** self-clean. After it fires, its cron matches nothing, but the row stays in the trigger list until something deletes it. It shows in pickers, the filter dropdown and `list_triggers` output. Pick one of two ways with the user before you create it:

1. **Leave it.** Tell the user it will sit in the trigger list after firing, and they can delete it from the UI. Don't promise to clean it up.
2. **Ask the trigger to delete itself.** Add a sentence in the user's voice to the intent, for example `"Send me a hello notification, then delete this trigger."` Don't name `delete_trigger` or paste in the trigger id. The engine's fire envelope already tells the running LLM its own id and that self-deletion is permitted. Then tell the user the trigger will delete itself after firing.

Never claim "I'll delete it after it runs" without one of the above.

## On-disk trigger definition (`trigger.toml`)

**The scheduler never reads this file.** `data/triggers/<slug>/trigger.toml` is a
**derived read-model** of the trigger's definition. It mirrors the durable
subset of its config (`name`, `slug`, `schedule`, `timezone`, `run`, `on`,
`app_id`, `go_to_review`, `group_id`, `side_effect_grant`, `model`,
`reasoning_effort`, `provider`). A trigger on the account chat defaults omits
the last three. The engine maintains it from the trigger events: written on
create/update, removed on delete, rebuilt from events on boot (ADR 0019).
Runtime and identity fields (`id`, `last_run`, `last_run_status`, `paused`) are
left out. It is **not version-controlled**: the engine adds
`data/triggers/*/trigger.toml` to the workspace repo's local `.git/info/exclude`.

Events are authoritative: the scheduler runs off the event-replayed config, and
this file mirrors that config, never feeds it. Two rules follow:

- **Never hand-edit it.** The edit writes a file that reads correctly and changes
  nothing the scheduler sees; the next trigger event or restart overwrites it.
  Change triggers via `create_trigger`/`update_trigger` (or the UI), which emit
  the events the projection follows.
- **Never verify from it.** After a config change, re-read the trigger from
  `list_triggers`, the live registration. Reading `trigger.toml` back off disk
  proves only that it was written, so a change the scheduler never saw still
  verifies green.

Each fire is recorded as events (`TriggerExecuted` + `TriggerCompleted`, plus any
*domain event* the run emits). The trigger's row in the triggers panel shows the
**last run's OK/failed status** next to its timestamp. There is no built-in
run-history view. For detail on a threadless trigger's runs (what it found, when,
why a run failed), ask the *Lucidos Agent*, which reads the events via
`query_events`. Or build an *app* on the trigger's events (`lucidos.events`).

The file exists so a trigger is inspectable (the Plugins panel's installed-plugin
file links point at it for plugin-shipped triggers) and so a *plugin* can SHIP a
trigger by declaring one, see `plugins.md`.

### Renamed trigger → stale `run.path`

The folder is named by the trigger's `slug`, never by its current `name`, so
renaming moves nothing. Changing the slug (an explicit `slug` via the CLI / HTTP
API, or a delete-and-recreate) relocates only `trigger.toml` to
`data/triggers/<new-slug>/` and deletes the old copy. `scripts/`, `knowhow/`
and the registered `run.path` all stay under the old slug. The tell: a
`trigger.toml`-only folder beside a `scripts/`-only one.

Repair, in order:

1. `git mv` the old slug's `scripts/` and `knowhow/` (whichever exist) into
   `data/triggers/<new-slug>/`.
2. `update_trigger(trigger_id, run={type:"script", path:"triggers/<new-slug>/scripts/run.py"})`.
   Only the event re-points the scheduler.
3. Confirm the new path in `list_triggers`, then delete the old folder. Deleting
   before step 2 lands removes the script the scheduler is still calling.

A broken run reports `Script not found: data/scripts/<path>`. That is the last
candidate in the path-resolution fallback, not the configured path, which
`list_triggers` shows.

## Setup checklist

1. **Set timezone first** if not already set. Cron is 6 fields (`second minute hour day-of-month month day-of-week`) in the user's local timezone, DST-aware via IANA tz. The `create_trigger` tool refuses without a timezone. For anything beyond a plain daily or weekly time, read § "Writing cron expressions" above: the AND/OR split, the nth-weekday and last-weekday recipes, and the combinations the engine rejects.
2. **`list_triggers` first** to check whether an existing trigger should be updated instead of creating a new one.
3. **Decide cron vs. `on` (and whether `on` needs multiple entries)** before writing the trigger.
4. **Write the knowhow file, THEN `run.intent` as the user would say it.** The ordering is the rule (§ "Write the knowhow file FIRST, then the intent"). Reusable recipes go in shared `data/knowhow/` (see `building-knowhow.md`); trigger-scoped ones go at `data/triggers/<slug>/knowhow/<descriptive>.md`. `<slug>` is minted from the name at creation (unless given), stored on `TriggerCreated`, and unchanged by a rename, so the folder keeps the old name. The LLM tools take no `slug`; the CLI (`lucidos triggers create --slug`) and HTTP API do. Changing it strands `knowhow/` and `scripts/` under the old slug (§ "Renamed trigger → stale `run.path`").

   There is no `run.knowhow` field: the trigger thread finds knowhow via its own `load_knowhow` calls, as chat does. The deserializer silently drops a legacy `run.knowhow:[...]` in old `TriggerCreated` payloads. Rewrite such an intent to name the knowhow inline ("see `system-knowhow/X`"). Or make it rich enough to nudge discovery from the system-prompt knowhow listing. Give the file precise `name` and `description` frontmatter so semantic discovery finds it.

   Shared `data/knowhow/` is what you can write first. Trigger-scoped is the
   exception to the ordering: write that one *after* `create_trigger` returns,
   because only then is `<slug>` authoritative.

## Common mistakes to avoid

- **Recreating instead of editing.** The biggest source of orphaned thread history. See "Edit, don't recreate" above.
- **Hand-editing `trigger.toml`.** The scheduler never reads it, so the edit no-ops and the next trigger event or restart overwrites it. Change the config with `update_trigger`, then verify against `list_triggers`. See "On-disk trigger definition" above.
- **Resuming a paused trigger to "run it now", or hand-rolling the run.** Resume runs nothing by itself. Use `triggers(action="run")`, or emit the subscribed event for an event-only trigger. See "Running an existing trigger once, off-schedule" above.
- **A trigger that manages another trigger.** Refused at fire time, and nothing catches it earlier. See § "What a trigger fire may not do to other triggers".
- **Recipe-in-text.** Procedure in `run.intent` instead of knowhow, almost always because the knowhow file was never written first. See "Write the knowhow file FIRST, then the intent" above.
- **A webhook condition without the `payload.` prefix.** `{"action": "completed"}` matches nothing, and nothing warns you. See § "The envelope: a webhook's body lands under `payload`".
- **Cron when a trigger subscription fits.** Polling burns runs and adds latency. If an event exists, prefer it.
- **Picking a trigger per event to maintain an aggregate without measuring first.** Weigh it against a projection. See § "Aggregating events: cron, per event, or a projection".
- **Assuming day-of-month and day-of-week are ORed.** They are ANDed, unlike Vixie cron. See § "Writing cron expressions".
- **A cron that can never fire.** The engine rejects impossible dates. Still read the next 3 fire times it reports on every create and update: they also catch expressions that fire far more rarely than the user meant.
- **Parallel triggers for one workflow that reacts to several events.** Use one trigger with multiple `on` entries.
- **No knowhow file for a procedure the trigger clearly needs.** The LLM then re-derives the procedure every run, slightly differently each time.
- **Vague `name`/`description` frontmatter on a trigger-scoped knowhow.** Discovery is semantic, not by id, so `notes.md` with `name: Notes` won't surface. Name the file by what it teaches (`openai-availability-check.md`), and write the `description` as the question that should retrieve it.
- **Knowhow that recommends raw `curl`/`fetch` for an API the workspace already proxies.** `curl -H "Authorization: Bearer $CRED_..."` (or a `requests`/`fetch` equivalent) leaks the credential into argv and tool transcripts. Use the `proxy_request` LLM tool against an entry in `data/config/apis.json` (`system-knowhow/building-knowhow.md` § "Calling external APIs from a recipe").
- **Notifying on every tick.** It trains the user to ignore notifications.
- **Two live triggers with the same name.** Filter pickers and notification deep-links can't tell them apart.
- **Promising behavior the trigger doesn't have.** Describe only what is configured. A one-shot does not self-clean (see "One-shot triggers" above).
- **Tool names or trigger ids in `run.intent`.** "Call delete_trigger with trigger_id <uuid>" leaks procedure and re-pastes the id the fire envelope already provides. Use the user's voice ("then delete this trigger").
