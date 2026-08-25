---
name: Triggers
description: Use when the user wants something to happen automatically ("every morning", "notify me when X happens", "watch for Y") or an EXISTING trigger to run right now ("run this trigger now", "fire it manually"). Load it even if a trigger may not be the answer: it settles that, and routes "tell me HERE when X happens" to `await_event` instead.
---

# Triggers

The working reference for *triggers*: choosing one, building it, editing it, and running an existing one off-schedule. Cron format, frontmatter, and field reference live in the engine system prompt and the grouped `triggers` tool description, so don't restate them. The CLAUDE.md "trigger intent vs. procedure" rule is summarized below; see `docs/taxonomy.md` § Triggers for the worked example.

> **Tool surface.** Triggers are managed through the grouped **`triggers`** tool
> (`action: create | list | update | delete | pause | resume | run`) and the grouped
> **`trigger_groups`** tool (`action: list | create | rename | reorder |
> delete`). Throughout this guide, a bare verb like `update_trigger` /
> `list_trigger_groups` is shorthand for that tool with the matching `action`
> (e.g. `update_trigger(trigger_id, …)` = `triggers(action="update",
> trigger_id, …)`). The old flat tool names still work as back-compat aliases,
> but the grouped tools are the surface the model sees. The CLI mirrors them as
> `lucidos triggers …` / `lucidos trigger-groups …` (see `lucidos-cli.md`).

## When a trigger is the right answer

| User says | Right answer |
|---|---|
| "Every morning, send me…" | Trigger (cron) |
| "Notify me when my package ships" | Trigger (`on`), with a separate event-emitting source |
| "When either X or Y happens, do Z" | One trigger with multiple entries in `on` — not two parallel triggers |
| "Check this once and tell me" | Just do it now, no trigger |
| "Remind me at 5pm today" | One-shot trigger (cron for today) — see "One-shot triggers" below |
| "Tell me **here** when X happens" | `await_event`, NOT a trigger. See the next section |

### First ask where the answer goes, not just how often

Both answers are the same *event subscription* underneath, `{event_type,
condition}` matched by the same code. They differ in WHO CONSUMES THE MATCH: a
**trigger subscription** spawns a new thread and stays armed for the next one, a
**thread subscription** resumes an existing thread and is spent once it fires.

A trigger runs in **its own thread**. It reaches the user as a notification, and
it cannot continue the conversation they are typing in. So "let me know **here**
when a coding agent edits code", "tell me in this chat when the build finishes",
or any request made inside a thread the user is plainly waiting in, is a thread
subscription (`await_event`), not a trigger. That holds **even when the phrasing
sounds like a standing rule**: "when X happens, tell me" reads as forever, but
what the user asked for is delivery into this conversation, and only
`await_event` does that. It costs nothing while it watches: the call returns
immediately, the turn ends normally, and the engine re-opens the thread when the
event lands, so the report arrives where they are reading. It is one-shot, so you
re-arm per event, and consecutive subscriptions are capped (the tool description
carries the number), which is exactly why an unbounded promise belongs to a
trigger instead.

Duration is the *second* question, and it is the one this whole file is about:
a reaction that must outlive the conversation, run when nobody is present, and
fire indefinitely is a trigger. Both answers are often right at once. Lead with
the one that matches where they asked to be told, and offer the other in the
same breath: "I'll watch and report here; want a trigger too, so it keeps
running once this thread is done?"

Do **not** build a trigger whose job is to post back into a chat thread.
`docs/adr/0047-event-wait-is-an-event.md` records that as precisely the
workaround `await_event` replaced: it costs a persisted trigger row per wait
(orphaned if the thread dies) plus a whole extra LLM turn in a *different*
thread just to route one message, and what lands is a fresh message that starts
a **new exchange** rather than resuming the turn that was waiting. The waiting
thread has no distinct state either, so it reads as finished.

If the user only wants it to happen **once, right now** — a check, a lookup, a computation — just do it inline; no trigger. But if the one-off is anchored to a **future time** ("remind me at 5pm", "ping me in 20 minutes"), it is NOT an inline task: you are not running at 5pm and nothing auto-resumes you, so an inline "reminder" is silently dropped. A future-time one-off needs a **one-shot trigger** (ideally self-deleting) — see "One-shot triggers" below.

## Write the knowhow file FIRST, then the intent

This is the rule most often got wrong, and it is got wrong because it used to be
stated as a prohibition. "Don't put procedure in `run.intent`" tells you what
not to write; a model holding a procedure and no other place to put it writes it
anyway. So the rule is an **ordering**, and it comes before the `triggers` call:

> **If the work has any procedure at all — a script to run, a flag to pass, a
> format to follow, a threshold to compute, a fallback, a file to read first —
> write that procedure to a knowhow file BEFORE you create the trigger. Then
> write `run.intent` in the user's voice, with none of it in there.**

Two steps, in that order, every time. The trigger thread looks knowhow up itself
by calling `load_knowhow` at fire time, exactly as a chat session does, so there
is no per-trigger allow-list to configure and nothing to wire: the file existing,
with a precise `name` and `description`, is the whole mechanism. Placement rules are in `building-knowhow.md`.

**A trigger-scoped file is the one you cannot write first.** Its path is
`data/triggers/<slug>/knowhow/`, and the tools take no `slug`: the authoritative
one exists only once the trigger has been created. Guess it and the file strands
in a directory no thread of that trigger ever reads. So put the recipe in shared
`data/knowhow/`, which is its right home unless it is useless to anything else.
If it truly is private to this trigger, create the trigger first, read the slug
back from the triggers list, then write the file before you reply.

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

Every clause after the first is procedure wearing the user's voice. Nothing was
written to knowhow, so the next thread that needs the recipe rediscovers it, and
a reader of the trigger config cannot tell what was actually asked for.

**Good**, two steps. First the knowhow file, at
`data/knowhow/build-health-report.md`. Shared, because that is the home you can
write before the trigger exists, and a second thread may well want the recipe:

```markdown
---
name: Build Health daily report recipe
description: How the daily Build Health report is produced — the collector, the
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

The HTTP API accepts a procedure-laden intent and will not stop you. See
`docs/taxonomy.md` § Triggers for the same split stated as taxonomy.

**Before you call the triggers tool, ask whether the work carries any procedure at all.** A script to run, a flag to pass, a format to follow, a fallback, a threshold to compute, a file to read. If it carries any of those, write them to a knowhow file FIRST, in shared `data/knowhow/`. Only then write `run.intent`, in the user's voice, with none of the procedure left in it.

The trigger thread inherits the same knowhow surface a chat thread has: the
system prompt's intent registry advertises what's available, and the LLM calls
`load_knowhow` when it judges a recipe relevant. Writing the procedure inline to
bypass that lookup turns the intent into a recipe, and the next person who reads
the trigger config can't tell what the user originally asked for.

## Cron vs. `on` vs. both

- **Cron** — "every morning at 8" / "weekdays at noon". Time-driven.
- **`on`**: "when X happens". Reactive. Each entry in the `on` array is a *trigger subscription*: an event type plus an optional payload filter. The event must already be emitted by something (an app, another trigger, an integration). A match spawns a new *trigger thread* and leaves the subscription armed for the next one, which is what makes a trigger a standing rule.
- **Both** — rare; usually means cron with a payload-shaped condition that should be event-driven instead. Re-examine before doing this.

If the user says "notify me when X" and X isn't an event yet, you have two work items: (1) make X emit an event, (2) trigger on it. Tell the user that explicitly.

**Check first whether the engine already emits it.** An `on:` entry takes a persisted thread event, a domain event your workspace emits, or a persisted **system event**: `BackupCompleted`, `BackupFailed`, `NotificationCreated`, `TriggerCompleted`, `PluginInstalled` and the rest (ADR 0113). "Tell me if a backup fails" needs no new emitter. What stays out is a transient frame such as `BackupProgress` or `Toast`, which writes no row and reaches no matcher. Subscribe to the event that ends the run instead. `system-knowhow/thread-events.md` § "Today the scheduler uses a blocklist" carries the full rule.

**Do not guess the name.** Create and update both check every `on:` entry. A misspelled or retired engine name is refused, with the real one named, because an exact-string match would arm clean and never fire. A name outside the engine's set is accepted, with a warning when this workspace has never emitted it. Look one up with the `events` tool's `event_types` action.

### One trigger, multiple events

The `on` field is a list. Use multiple entries when *one workflow* should react to several event types — e.g. "summarize my day on `MessageReceived` from my partner OR on `EmailReceived` from my boss". Two parallel triggers with the same intent is a UX trap: editing one and forgetting the other silently drifts behaviour.

Each entry carries its own `condition`, scoped to *that* event:

```json
{
  "on": [
    { "event_type": "OuraSleepImported", "condition": { "sleep_score": { "$lt": 70 } } },
    { "event_type": "EmailReceived" }
  ]
}
```

The `sleep_score` filter does NOT apply to `EmailReceived` — its payload doesn't have that field at all. Per-entry conditions mean different event payload shapes never constrain each other.

### Aggregating events: cron, per event, or a projection

"If an event exists, prefer it" is about how you learn something happened. It does not settle how to maintain an **aggregation**: a rollup, a running total, a per-period summary, a counter, anything whose value is current state derived from N events. Both shapes are legitimate, and each costs something the other does not.

- An **event subscription** buys freshness, and costs one run per event.
- A **cron** buys a bounded, predictable cost, and costs staleness of up to one interval.

Which of those matters more is your call, and it turns on the consumer: what it does with the number, and how wrong the number is allowed to be between updates. What follows is how to answer that for your own case, not a verdict.

**Often the answer is a projection rather than either trigger shape.** When an event type is firing a lot and something needs to consume it, folding each event into a maintained read model and pointing consumers at that model tends to beat having every consumer re-aggregate raw events. The projection is the artifact: a stored value plus the cursor recording how far it has consumed. What advances it (a cron recompute, an O(1) per-event increment, a reader that merges the tail itself) then becomes a separate and much smaller question. The shape worth thinking twice about is a trigger per event on a busy event class, because **a trigger fire is a thread, not a callback**: each one goes through thread-queue admission and spawns a real process, and that fixed cost does not shrink as the fires get denser.

Two questions worth measuring before picking:

- **Does the per-fire work actually shrink as the fires get denser?** Split the measured per-run cost into fixed overhead (process start, connection), the part proportional to NEW rows, and the part proportional to the WINDOW recomputed. Only the middle term shrinks. If a run is dominated by the window term, firing more often multiplies a near-constant, and nothing about the code makes that visible until it is measured.
- **Can you keep up at the PEAK rate, not the average?** Multiply peak rate by per-fire cost. Above one unit of work per unit of wall clock the fires cannot drain, and event fires are never coalesced the way cron fires are: with `max_concurrent_per_trigger` at 1 they queue strictly FIFO, the backlog runs into `max_queued_per_trigger` (25), and `overflow` drops the oldest waiting fires. Averages hide this, and a projection that breaks at the peak stays broken. See `system-knowhow/thread-queue.md`.

One consideration that is not about cost: **a whole-window recompute is idempotent and self-healing.** Rerun it and nothing changes; miss a run and the next one repairs the gap. An incremental per-event append is neither: a rerun double-counts, and a missed fire is silent drift with nothing to detect it. That is a reason to lean toward recompute, or to pair an incremental path with a reconciling recompute underneath, rather than a reason to rule incremental out.

**The one hard rule here, and the exception to the surrounding guidance: a trigger must not subscribe to an event class its own run emits.** That is a feedback loop, not a tradeoff. The concrete case is an *intent* trigger on any LLM-activity event: its own model call emits the event it subscribes to. Only the script flavour is even arguable on that class of event.

The engine backstops this rather than preventing it. An event a run emits dispatches one level deeper in the chain, and `MAX_EVENT_TRIGGER_DEPTH` (3) stops the chain there. So the loop ends after three fires instead of never. Three fires of an opus trigger is still a bill you did not mean to pay. That is why this is a rule, not a setting.

Shapes worth knowing, roughly in the order they tend to fit:

1. **A cron for the projection, with the consumer merging the recent tail itself.** It queries the event store for rows above the projection's stored cursor and folds them on top, so the reader is current without the projection being current. This is what the Token Cost dashboard does.
2. **O(1) per-event work with no database round trip**, when the projection has to be current between runs. Read the row out of `TRIGGER_EVENT_PAYLOAD`, which the engine already hands the script, increment, write. A cron underneath as a reconciling rebuild buys back the self-healing above.
3. **The plain event trigger**, when the event is low-volume or user-initiated. The hybrid shape is this one plus a floor: a cron, plus `on: SomethingRequested` for a Refresh button.

A rate to read as a smell rather than a threshold: an event firing more than roughly once a minute sustained is worth measuring before you subscribe a trigger to it. The number settles nothing by itself, it is just where the two questions above start coming back with different answers. `system-knowhow/thread-events.md` § "Volume classes" labels each engine event, and `lucidos events count` gives a workspace's actual rate.

**Worked example**, offered as an illustration of running that measurement rather than as the source of any threshold (a live workspace, August 2026). `ContextCaptured`, one row per model API call, ran 7,500 to 17,600 rows a day, peaking at 2,415 in one hour and 238 in a single minute. Its rollup script took 3.4 seconds per run. Everything below is that 3.4 seconds multiplied out. Per event it is 7 to 17 hours of query time a day (7,500 and 17,600 runs) to maintain a projection that costs about 82 seconds a day as an hourly cron (24 runs), and at the peak minute it would need just over 13 minutes of work (238 runs) to service 60 seconds of events. The split is what made the choice obvious: process start 29 ms and psql connect 25 ms, both negligible, with the whole 3.4 seconds sitting in one SQL query whose window function scans each touched thread's full history, which does not get cheaper when you fire more often.

## Writing cron expressions

Six fields, `second minute hour day-of-month month day-of-week`, in the user's local timezone. Two rules decide what a trigger actually fires on, and they pull in opposite directions:

- **Within one expression, the fields are ANDed.** Every field must match. So when day-of-month AND day-of-week are both set, the expression fires only on days that satisfy both. (Vixie cron ORs those two specific fields. Lucidos does not. Never write a cron on the Vixie assumption.)
- **Across the array, the expressions are ORed.** A trigger's `cron` takes a list, and it fires at the earliest match from any entry. This is how you express "either of these".

Both rules are load-bearing for the recipes below. Neither is a bug, and neither is going to change.

### The footgun: one expression is not "either"

`0 0 9 1 * Mon` reads to almost everyone as "the 1st, plus every Monday". It means "the 1st, but only when the 1st IS a Monday". That happens about 1.7 times a year on average and in lumpy gaps: it fires 2026-06-01, then nothing until 2027-02-01, then 2027-03-01, then nothing until 2027-11-01.

"The 1st, plus every Monday" is two expressions:

```json
{ "cron": ["0 0 9 1 * *", "0 0 9 * * Mon"] }
```

The engine warns (without refusing) when a single expression restricts both fields in a shape that fires rarely. It stays deliberately quiet for the 7-day windows below, which use the same AND on purpose.

### Day-of-week numbering

Write day-of-week in standard cron numbering: 0 and 7 both mean Sunday, 1 is
Monday, and 6 is Saturday. The engine translates this for you before parsing
the schedule.

Underneath, the `cron` crate numbers days 1 (Sunday) through 7 (Saturday).
`translate_dow_for_cron_crate` in
`crates/lucidos-engine/src/engine/tools/scheduler.rs` rewrites each plain
numeric day, or a plain `a-b` range, as `(n % 7) + 1` before the crate ever
sees it. You never write the crate's own numbering yourself.

| Day | Write (standard) | `cron` crate sees |
|---|---|---|
| Sunday | 0 or 7 | 1 |
| Monday | 1 | 2 |
| Tuesday | 2 | 3 |
| Wednesday | 3 | 4 |
| Thursday | 4 | 5 |
| Friday | 5 | 6 |
| Saturday | 6 | 7 |

Named days (`Mon`, `MON-FRI`, `SAT,SUN`) bypass translation. The crate
numbers names Sunday-first too, so a plain named day or an ordinary named
range is safe as written. This check runs per comma segment. A mixed field
(`Mon,1`) still shifts the numeric segment: `1` becomes Monday, same as
`Mon`.

`translate_dow_for_cron_crate` also leaves an out-of-range numeric token
(`8`, `999`) untranslated, on purpose, and the crate rejects it. You get a
parse error, never a silent wrong day. A numeric range-and-step token
(`1-5/2`) fails validation, since the engine cannot shift it safely, even
inside a mixed field like `Mon,1-5/2`. Write the days out instead (`1,3,5`),
or use the named range form (`Mon-Fri/2`), which fires on the days written.

A range that crosses Sunday fails in both forms. `5-0` (Friday through
Sunday) becomes `6-1` after translation. `6-1` (Saturday through Monday)
becomes `7-2`. Both have a start above their end, and the crate rejects that
shape outright. The named form fails the same way: `Fri-Sun` numbers to
`6-1` internally and hits the identical check. For a range that crosses
Sunday, list the days instead of ranging them (`5,6,0` or `Fri,Sat,Sun`), or
split into two cron expressions.

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

Verified against every month from 2026 to 2100 (900 months): exact in 898, with zero double-fires. The two misses are **February 2044 and February 2072**, the only leap years in that range where Feb 29 falls on a Monday; there it fires Feb 22 instead. Adding `0 0 9 23-29 2 Mon` as a fourth expression fixes those two months but makes them fire **twice** (the 22nd and the 29th), so three is the better trade. Say so when you build one, rather than hiding the edge.

Swapping the weekday gives last Friday, last Tuesday and so on, with the same shape and its own leap-February exception (for Friday it is 2036, 2064 and 2092). For plain **month end** with no weekday, pin the last day per month-length class instead: `["0 0 9 31 1,3,5,7,8,10,12 *", "0 0 9 30 4,6,9,11 *", "0 0 9 28 2 *"]`. February is the awkward one either way, since its last day moves; `28` is a day early in leap years, and `28,29` fires twice in them. Pick one with the user.

### Expressions that can never fire

A day-of-month that the month is never long enough to contain is syntactically valid and semantically dead. These parse cleanly, and before the guard they were accepted, registered, and shown as healthy while doing nothing forever:

| Expression | Why it never fires |
|---|---|
| `0 0 9 31 2 *` | February has no 31st |
| `0 0 9 30 2 *` | February has no 30th |
| `0 0 9 31 4,6,9,11 *` | April, June, September and November have 30 days |
| `0 0 9 30 2 Sun` | impossible date; the weekday is irrelevant |

**The engine now rejects all of these at create and update**, naming the offending fields (`day-of-month 31 never occurs in month 2 (February)`). So you will get an error rather than a silently dead trigger. Fix the expression; there is nothing to work around.

`0 0 9 29 2 *` is NOT in this class. Feb 29 is rare, not impossible: it fires 2028, 2032, then 2036. Every create and update reports the **next 3 fire times** back to you, and the trigger's row in the panel shows them too. Read them against what the user asked for before you confirm: three dates a year apart when they said "monthly" is the tell.

## `condition` — when to filter

Set `condition` on a trigger subscription when the event is high-volume and you only care about a slice. Example: subscribe to `EmailReceived` but only fire on emails from a specific sender. Without a condition, the trigger fires for every email and the LLM has to filter inside the run, which is wasteful and slow.

Don't use `condition` for logic that depends on external state (e.g. "only if this app's data file says X"). Conditions are pure payload filters. Stateful checks belong inside the run.

**One field is always available on a thread event: `thread_id`.** It is not in any event's payload: the engine supplies it from the thread the event belongs to. It scopes a subscription to a single thread, so `{ "event_type": "CodingAgentIdled", "condition": { "thread_id": "<uuid>" } }` fires only when THAT coding-agent session reaches a turn boundary. A **domain event** (one your workspace emits with `emit_event`) belongs to no thread. It has no such field, so a `thread_id` condition on one matches nothing. Everything else a condition names is a **field path** into that event's own payload.

A persisted **system event** is the same case. `BackupFailed` belongs to no thread, so a `thread_id` condition never matches it. Condition on the variant's own fields instead, such as `filename` on `BackupCompleted`. The stored row wraps the event in a `type` / `data` envelope, and the matcher unwraps it for you, so never name those two keys.

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

**Nothing warns you, and that is what makes this expensive.** A path that resolves to nothing is null. So is a field that is present and null, and the matcher cannot tell them apart. A subscription that can never match looks exactly like one that is patiently waiting. The trigger arms clean, its panel row stays healthy, and `last_run` keeps the timestamp of the last real fire.

**So diagnose by comparison, not by reading the panel.** Query the event store for the event type you subscribed to, and hold its newest row against the trigger's `last_run`. Deliveries arriving with no runs beside them is the tell.

**The general rule: write the condition against the STORED event, not the upstream payload you think you are subscribing to.** Only the shape of the row in the event store decides what a field path resolves to. So read one real stored event first, with the `events` tool's `query` action and `limit` 1, then write every path from what you see. A path copied out of the sender's API docs is a guess.

**Script triggers inherit the same envelope.** `TRIGGER_EVENT_PAYLOAD` holds the whole event payload, wrapper included, so a script has to reach through `payload` too. A hand-emitted test event is usually written flat, so a script can pass your test and still fail on the real delivery. Read it defensively:

```python
raw = json.loads(os.environ.get("TRIGGER_EVENT_PAYLOAD", "{}"))
body = raw.get("payload") if isinstance(raw.get("payload"), dict) else raw
```

## Notification discipline

`send_notification` only fires when there's something the user actually wants to hear about. A morning summary that finds nothing new should produce no notification — silent success is the norm.

The scheduler auto-creates an error notification when a trigger fails. Don't double-notify on errors from inside the run.

## Where the thread lands: `go_to_review`

By default, trigger runs are unattended — their threads go straight to Archive when they finish, and only surface in the Current section if the user follows up with a message. This is right for most cron triggers (silent imports, periodic syncs, idle nudges).

Set `go_to_review: true` when the trigger's *output is the point* — a daily summary the user is meant to read, an alert that needs acknowledgement, a scheduled report. The thread then surfaces in the Current section on completion so it's not lost in Archive.

| User phrasing that answers it | Flag |
|---|---|
| "import my data", "sync X", "keep Y up to date" — silent housekeeping | omit (default false) |
| "put it in front of me", "make sure I see it", "I want to read this" | `go_to_review: true` |
| "summarize my week", "write a report I should look at" — output is the point | `go_to_review: true` |

A `send_notification` does **not** answer this question — notifications and review-surface are independent. A "notify me when X" trigger may or may not also need its thread to surface in review; the user has to tell you which.

If the user's request doesn't clearly land in one of the rows above, **ask** — see Question 5 below. The flag is snapshotted onto each run when it fires; toggling it later only affects future runs.

## Which model the run uses: `model` and `reasoning_effort`

An intent trigger fires on the account chat defaults (Settings → Models →
Chat & triggers) unless it says otherwise. Set `model` to pin it to a specific
chat model and `reasoning_effort` to pin its thinking budget
(`none|low|medium|high|xhigh|max`); omit either, or send null, to go back to the
account default. The two are independent: pinning the model leaves the effort on
the account setting, and the reverse.

Script triggers have no model. They run no LLM, so both fields are ignored there
and the form hides them.

| User phrasing | What to set |
|---|---|
| "use something cheap for this", "it's just a digest" | a low-cost model, often with a low `reasoning_effort` |
| "this one needs to be thorough", "use the best model" | the stronger model, and usually a higher `reasoning_effort` |
| nothing about models | omit both, so the trigger follows the account default |

Only pin a model when the user asked for one. A pinned trigger stops following
the account default, so a workspace-wide model change no longer reaches it,
which is the point when it is deliberate and a surprise when it is not.

The model id is **not checked against the registry when you save**, the same as
the `chat_model` preference: a model can be disabled or deleted long after the
trigger was written. A wrong id therefore fails at fire time, as a normal
trigger-failure notification, not at save time. Use `manage_models(action='list')`
to see the real ids.

The model and effort a run actually used are recorded on its `TriggerStarted`
event, so the trigger's thread shows what it ran on and a follow-up there
continues on the same model rather than snapping to the account default.

### Notification routing (`app_id`, `tap`, `event_id`)

Three independent fields control the notification:

- **`app_id`** — *which* app the notification is about. Drives the inbox modal's "Open <app>" button. Set it whenever the notification relates to a specific app (so the user can navigate from the modal to the relevant app), even when the tap routing is `{ kind: 'modal' }`.
- **`tap`** — *what happens on tap*. Discriminated union: `{ kind: 'modal' }` (default — opens the inbox detail showing the body; use it for informational pushes too, every notification is openable) or `{ kind: 'navigate', to: NavigateUi }` (delegates to the same router `navigate_ui` uses; `to` is its arg shape). Both mark the source notification read on tap. (The passive `{ kind: 'none' }` kind was retired — `docs/plans/2026-07-02-remove-notification-tap-none.md`.)
- **`event_id`** — *which specific event inside the linked thread* raised the notification. Optional UUID. Used by the §4 in-app matrix to silently mark-read when the user is already looking at the source event. Distinct from `tap.to.event_id` (which is the scroll-and-pulse target when the tap navigates to a thread — typically the same value).

Write the **`message` as content only — never restate the `title` in it.** Every surface renders the title in its own right (the in-app toast promotes it to the heading, the inbox detail to its `<h2>`, the OS push to the banner title), so a body that opens by repeating the title shows it twice. Use a bare sentence for a single item and `"• "`-prefixed lines for a list; the toast renderer picks those up as bullets under the title. See `system-knowhow/notifications.md` §4.

| Trigger says | `app_id` | `tap` | `event_id` |
|---|---|---|---|
| 8:00 habit-tracker "Check in for today" — direct CTA inside an app | habit-tracker | `{ kind: 'navigate', to: { target: 'app', app_id: 'habit-tracker' } }` | — |
| Coding agent is asking the user a question — needs them back in the conversation, on that question | omit | `{ kind: 'navigate', to: { target: 'thread', id: '<thread_id>', event_id: '<event_id>' } }` | source event id |
| Coding agent is asking for permission — same idea, different event | omit | `{ kind: 'navigate', to: { target: 'thread', id: '<thread_id>', event_id: '<event_id>' } }` | source event id |
| "5 changes ready to apply" — multi-item panel destination | omit | `{ kind: 'navigate', to: { target: 'changes' } }` | — |
| Daily summary "you completed 5 tasks today" — informational, no CTA | omit | `{ kind: 'modal' }` (default) | — |
| 22:00 bedtime nudge — informational | omit | `{ kind: 'modal' }` (default) | — |
| Habit-tracker weekly report — about an app, but the action is reading | habit-tracker | `{ kind: 'modal' }` (default) | — |
| "Backup complete" / "Sync finished" — purely informational, no action needed | omit | `{ kind: 'modal' }` (default) | — |

Tap defaults to `{ kind: 'modal' }` so the user reads the message and decides what to do — `navigate` is the explicit opt-in for direct CTAs and panel deep-links. Informational pushes use the default `{ kind: 'modal' }` too — every notification is openable (the passive `{ kind: 'none' }` kind was retired). The notification always lands in the inbox regardless of `tap`, so the user can re-open the detail manually from the bell icon.

See `system-knowhow/js-sdk.md` § `lucidos.notifications` for the full `NavigateUi` target list (panels, apps, threads, files, triggers, creation forms, URLs).

#### Where the LLM finds `event_id`

When a trigger fires from a `BusEvent::Thread` match, the engine appends a `## Triggering Event` block to the trigger's user message. Above the JSON payload, a line like:

```
Source event id: 7a9c2c5f-…
```

…carries the UUID of the event that fired the trigger. Pass that value to `send_notification`'s `event_id`. The push tap then deep-links to the exact event the trigger was about — the question card pulses on land, no scrolling needed.

For schedule (cron) triggers there is no source event, so no `event_id`. For on-event triggers that notify about *a different* event (e.g. fire on `CodingAgentIdled` but notify about the last `UserQuestionAsked`), look the right event up yourself with `query_events` and use that id.

#### Worked example: push when agent needs me

```yaml
on:
  - event_type: UserQuestionAsked
run:
  intent: "Notify me when the agent has a question waiting for me. The push should deep-link straight to the question — tapping it takes me to the originating thread and pulses the question card on land."
```

The same shape works for `event_type: CodingAgentPermissionRequest` (swap the message to read from the `tool_name`/`summary` fields). Lucidos does not seed this trigger — workspaces opt in by creating it.

## Script triggers: when an LLM call is overkill

A trigger's `run` can be either `{ "type": "intent", "intent": "…" }` (the LLM path everything above describes) or `{ "type": "script", "path": "triggers/<slug>/scripts/run.py" }` (a script invoked directly with no LLM). Pick `script` when the work is mechanical — a fixed shape applied to whatever event(s) the `on:` list selects, a scripted API call, a deterministic emit — and an LLM judgement call isn't the feature.

Good candidates for `script`:

- "On any event in `on:`, notify with title + message read from the payload's common fields."
- "Every morning at 7, hit `<API>` and write the response to `data/artifacts/<date>/x.json`."
- "On `OrderPlaced`, emit `OrderQueuedForShipping` if `order.total > 100`."

Bad candidates for `script` (keep these as `intent`):

- Anything that needs to read the workspace's intent registry / knowhow library to pick a procedure.
- Anything where the message wording should adapt to context (the LLM's judgement is the feature).
- Multi-step workflows whose branches depend on prior results — the LLM-as-coordinator is what makes them work.

### Scripts run in place — `__file__` is the real path

The engine executes a registered script **from its real location on disk**, with the workspace root as the working directory. So `__file__` is `<workspace>/data/triggers/<slug>/scripts/run.py`, and the ordinary way of reaching a sibling directory works:

```python
_STATE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "state")
```

That resolves to the real `data/triggers/<slug>/state/` — the natural home for a script trigger's own state (a last-seen id, a per-version marker, a cursor). Prefer `__file__`-relative paths over paths relative to the working directory: they keep the script correct no matter who invokes it, and they keep its state beside the script that owns it, per the ownership rule in `docs/taxonomy.md`.

### Script trigger env vars

When the engine fires a script trigger that subscribes to a domain event, it sets the following env vars before exec'ing the script. Schedule fires emit none of them — the script has no source event to point at.

| Env var | Set when | What it holds |
|---|---|---|
| `TRIGGER_EVENT_TYPE` | Always on event fires | The matched event name (e.g. `UserQuestionAsked`). Use as a fallback title or when the script genuinely needs to branch on type. |
| `TRIGGER_EVENT_PAYLOAD` | Always on event fires | The source event's payload, serialized as JSON. Parse with `json.loads(os.environ["TRIGGER_EVENT_PAYLOAD"])`. |
| `TRIGGER_EVENT_ID` | When the source event has a row id | The `events.id` (UUID) of the source row. Pass to `lucidos notify --event-id` so the push tap scroll-and-pulses the exact card. |
| `TRIGGER_EVENT_THREAD_ID` | Only for *thread-scoped* source events | The thread the source event lives on. Pass to `lucidos notify --tap navigate --thread-id` so the push deep-links to the originating conversation instead of the trigger's own thread (which is `LUCIDOS_THREAD_ID`). |

The trigger's own thread is `LUCIDOS_THREAD_ID` (same env var every spawned subprocess gets). `TRIGGER_EVENT_THREAD_ID` is the *source* event's thread — these are different threads. A script that mixes them up will deep-link the push into the trigger's own (uninteresting) thread instead of where the user actually needs to act.

### Worked example: push when any subscribed event fires

The script is *event-agnostic*: the trigger's `on:` list owns which events fire it; the script just consumes whatever arrives. Add or remove events from `on:` and the same script keeps working.

`data/triggers/when-agent-needs-me/scripts/run.py`:

```python
#!/usr/bin/env python3
"""Push a deep-linking notification for any event the trigger subscribes to.

The trigger's `on:` list decides which events fire this — the script
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

Want to also notify on `EmailReceived` from your boss? Append another `on:` entry — the script doesn't need to change. The `run.path` is workspace-relative; the engine resolves it under `data/`. Swapping `intent` for `script` drops one LLM call per fire with no behaviour change visible to the user.

If a payload doesn't carry the well-known fields, only the fallback title (the event type) and a generic message fire. That's the cost of the event-agnostic shape; the alternative — branching on `event_type` inside the script — is a maintenance trap (every new event the user subscribes to also needs a script edit). Prefer carrying `title` / `message` in the payload at the *event's* emit site so any subscriber can render it cleanly.

## Grouping triggers

A *trigger group* is a user-visible folder shown as a collapsible section in the triggers panel. Groups are pure labels — they have no schedule, run no code, and don't coordinate firing. Their only job is to collect related triggers under one header so the panel stays readable.

Use a group when several triggers form an emergent workflow (one trigger emits an event via `emit_event`, another listens via `on_event`) and the user benefits from seeing them together. You don't need a group for a single trigger; the "Ungrouped" section at the bottom of the panel handles that case.

| Tool | When to use |
|---|---|
| `list_trigger_groups` | Before assigning a trigger, check whether a fitting group already exists. |
| `create_trigger_group(name, order?)` | Create a new section header. Names are unique within the workspace (case-insensitive). |
| `create_trigger` / `update_trigger` with `group_id` | Assign a trigger to (or move it between / out of) a group. `update_trigger(group_id: null)` clears membership. |
| `rename_trigger_group(group_id, name)` | Rename the section. |
| `reorder_trigger_groups([{id, order}, ...])` | Batch-reorder panel sections. |
| `delete_trigger_group(group_id)` | Refused if the group still has members — move or delete them first (the error response lists them). |

Groups are orthogonal to `app_id`. An app-owned trigger can live in any group; the engine doesn't auto-couple the two. `app_id` drives notification deep-linking; `group_id` drives panel layout.

## Side-effect grant — authorizing unattended risk

This matters **only when the workspace has the command guard on** (Settings → Permissions → Command safety; off by default). When it's on, the command guard classifies every `run_bash` / `run_python` command a trigger's intent runs. Most commands (reads, data crunching, downloads, writes inside the workspace) run untouched. But an **irreversible** one is gated: sending email, a mutating HTTP request (POST/PUT/DELETE), a cloud-CLI change (`gh`/`aws`/`gcloud`), destroying files outside the workspace.

A chat turn would *ask* the user to approve such a command. A trigger fires unattended: there's nobody to ask. So instead the trigger carries a **side-effect grant** — the set of irreversible side-effect categories it's pre-authorized to perform. At fire time:

- the command's side-effect category **is in the grant** → it runs;
- it **isn't** → the command is blocked and **the whole trigger run fails** (a failure notification surfaces it, naming the blocked command and the missing grant, with an *Open trigger* button that lands on these settings).

The categories are: **email**, **external API** (mutating HTTP), **cloud CLI** (gh/aws/gcloud), **out-of-workspace destruction**, and **other** (anything irreversible that fits none of the above). The default grant is empty — a new trigger may perform *no* irreversible side-effect.

**The grant is set by the user, not by you.** The `create_trigger` / `update_trigger` tools do **not** accept a grant field, and that is deliberate: an autonomous agent can't widen its own unattended authority. The user grants side-effects in the trigger's settings UI (the "Allowed side-effects" checkboxes). So when you build a trigger whose intent needs an irreversible side-effect (e.g. "email me the digest every morning"), **tell the user** they must tick the matching side-effect (here, *Send email or messages*) in the trigger's settings; with command safety on, the run otherwise fails the first time it tries to send. If command safety is off, none of this applies and the command runs unguarded.

**The grant also flows to coding-agent work the trigger spawns.** When a trigger's intent launches a *coding-agent thread* (Claude Code / Codex), directly or via a sub-thread an orchestrator spawns, that thread runs **unattended**, with no human to answer the coding agent's permission cards. Instead of hanging on a card forever, the engine resolves each request from the same side-effect grant (it walks the spawn tree to its root trigger and inherits that trigger's grant): benign in-workspace work (reads, in-workspace edits, git, `lucidos data write` to `data/`) is auto-allowed, an irreversible side-effect is allowed only if its category is in the grant, and a catastrophic command is always denied. Unlike the chat command guard (which fails the *whole* run on an ungranted side-effect), the coding-agent path denies just the one request, and the agent gets the denial and works around it or reports the step failed. This is independent of the command safety toggle. So a coding-agent trigger that needs, say, a mutating HTTP call still needs the user to tick **Call external APIs** on the trigger; otherwise that one call is denied (the rest of the run proceeds). See `coding-agent-events.md` § "Unattended auto-resolution".

## Edit, don't recreate

**Always look for an existing trigger first** (`list_triggers`) and modify it with `update_trigger`. Only call `create_trigger` when no comparable trigger exists. Recreating gives the new trigger a fresh `trigger_id`, which orphans the entire run history of the old one — the threads still exist in the database but no longer match the live trigger in the filter dropdown, in trigger-scoped reports, or anywhere else that joins by id. The user sees "no threads for current trigger" even though their workflow has been firing for months.

This applies to every shape of change:

| User says | What to do |
|---|---|
| "Change the cron to 9am" | `update_trigger(trigger_id, cron=...)` |
| "Rename it to X" | `update_trigger(trigger_id, name="X")` |
| "Switch it to fire on event Y instead" | `update_trigger(trigger_id, cron=null, on=[{event_type:"Y"}])` |
| "Also fire when Z happens" | `update_trigger(trigger_id, on=[existing..., {event_type:"Z"}])` — append to the `on` array, don't make a sibling trigger |
| "Stop firing on event Y" | `update_trigger(trigger_id, on=[existing... minus Y])` — `on` is a full replacement |
| "Tighten the Y filter" | `update_trigger(trigger_id, on=[..., {event_type:"Y", condition:{...}}, ...])` — replace that entry inside the full list |
| "Tweak the prompt" | `update_trigger(trigger_id, run={...})` |
| "Pause it" | `pause_trigger(trigger_id)` (or `update_trigger(..., paused=true)`) |
| "Make sure I see this one" / "Send to review" | `update_trigger(trigger_id, go_to_review=true)` |
| "Stop bringing this up — keep it in the archive" | `update_trigger(trigger_id, go_to_review=false)` |
| "Add another time it should run" | `update_trigger(trigger_id, cron=[existing..., new_expr])` — append to the cron array, don't make a sibling trigger |
| "Run it once more, like at 7pm tonight" | `update_trigger(trigger_id, cron=[existing..., one_shot_expr])`, then a follow-up `update_trigger` after it fires to remove the one-shot row. Don't create a duplicate trigger — even temporarily |

If you genuinely need a different trigger (different *workflow*, not a tweak of the same one), give it a clearly different name. Two live triggers named identically are a UX trap — the user can't tell them apart in any picker.

## Running an existing trigger once, off-schedule

**`triggers(action="run", trigger_id)`.** That is the whole answer for a cron trigger. The CLI is `lucidos triggers run --id <uuid>`, the SDK is `lucidos.triggers.run(id)`, and the trigger's row in the panel has a **Run once** button. (Not to be confused with the Thread Queue panel's **Run now**, which force-admits an entry that is *already queued* and cannot create a fire.)

**When you send the user to that button, link the TRIGGER, not the panel**: `[Nightly digest](trigger:<id>)`, with the id from `list_triggers`. The link lands on the trigger's own row, which is where **Run once**, the pause toggle and the last-run status are. `[Triggers](triggers)` opens the list and leaves them to find the row themselves.

It is a real fire, so it records `TriggerExecuted` / `TriggerCompleted` and the panel's `last_run` and OK/failed status, and it runs under the trigger's own identity, its side-effect grant, and (for an `intent` run) its *trigger thread* and `go_to_review` routing. Downstream nothing distinguishes it from a scheduled fire, deliberately. It returns as soon as the run is admitted, not when the run finishes.

Three answers other than "started", each of which you must relay as-is rather than reporting a run:

- **Already running.** A fire of this trigger was already active or queued, so nothing new started: scheduled fires coalesce to at most one pending run per trigger. Tell the user that; do not claim you started one.
- **Paused.** Refused. Resuming does not run anything *on purpose*, so if the user wants both, do both. (It can still fire something incidentally: re-registering the schedule also re-runs the missed-slot catch-up, so a cron slot from the past hour that never ran fires on resume. That is a side effect of restoring the schedule, not a way to ask for a run, and you cannot predict it.) A pause *you* just made counts immediately: every trigger write is visible to the very next call, so `pause_trigger` followed by `run` in the same turn is refused rather than raced, and there is nothing to wait for in between.
- **No cron schedule.** Refused, because a payload-less fire is a shape an event-only trigger has never had (an intent run would find no `## Triggering Event` block, a script run would get none of the `TRIGGER_EVENT_*` vars). Emit its event instead.

### Event-only trigger: emit the event

`events(action="emit", …)`, or `lucidos events emit <Type> --summary "…" --payload '{…}'` from a script. The emit goes through the same matcher, the same admission, and the same run as a real event, so this is the faithful reproduction.

- **Per-entry `condition` filters still apply.** A payload that fails the condition matches nothing and you get silence, not an error. Read the `on` array from `list_triggers` and build a payload that passes.
- Shape the payload like the real emitter's, not just enough to match: the run reads it (`## Triggering Event` for an intent, `TRIGGER_EVENT_PAYLOAD` for a script).
- The event is real and persisted, so every *other* subscriber fires too.
- **Event fires do NOT coalesce.** Unlike the run action (cron fires collapse to at most one pending run per trigger), event fires keep strict FIFO because each carries its own payload. Emit twice and the trigger runs twice, the second surfacing as an unexplained extra run minutes later. Check `list_threads` (rows carry `trigger_id` and `status`) before re-emitting.

### Don't imitate the fire

Copying `run.intent` into `run_thread`, or running the trigger's script yourself with `run_python` / `run_bash`, produces something that looks like a run and isn't one: no `TriggerExecuted`, no `last_run`, nothing in the trigger's history, none of the trigger-fire framing or its system rules, and no side-effect grant. Doing the work inline in the conversation thread is worse still: no per-run thread for the user to open, and a long or destructive procedure runs inside a chat turn.

Both stay fine for **debugging** ("does the script still crash?"), as long as you call it that. A hand-run script gets none of `TRIGGER_EVENT_TYPE` / `TRIGGER_EVENT_PAYLOAD` / `TRIGGER_EVENT_ID` / `TRIGGER_EVENT_THREAD_ID`, so an event-driven one raises `KeyError` on the first lookup, and `LUCIDOS_THREAD_ID` points at your conversation, so any `lucidos notify` lands in the wrong thread.

## Questions to settle with the user before creating

Don't call `create_trigger` from the user's first message. Most "create a trigger for X" requests leave at least one of these unsettled — confirm before writing the trigger. Skip questions only when the user has already answered them in the same turn.

1. **Recurring or one-shot — and if one-shot, now or at a future time?** Triggers are for things that should keep happening, so a recurring need is always a trigger. A one-off splits by *when*: if it's "do this **now**" ("check X and tell me"), handle it inline — no trigger. If it's anchored to a **future time** ("remind me at 5pm today", "ping me in 20 minutes"), it CANNOT be handled inline — you are not running then and nothing auto-resumes you, so an inline reminder is silently dropped — so it needs a **one-shot trigger** (cron for that time, ideally self-deleting). Whenever you create a one-shot (a future reminder, or an explicit test like "fire once in 2 min"), ask whether it should delete itself after firing — it won't on its own. Create it with `go_to_review` omitted (so the fire-thread lands in Archive, not the Current section) unless the user explicitly wants to read the run afterwards. See "One-shot triggers" below for the procedure.
2. **Cron or `on`?** "Every morning at 8" is cron. "When my package ships" is a trigger subscription. If the user names several events the same workflow should react to ("when X *or* Y happens"), they belong in one trigger with multiple `on` entries, not parallel triggers. If the event doesn't exist yet, name the work (emit the event from somewhere, then trigger on it) and confirm.
3. **What's the run.intent in the user's voice?** One sentence the user would actually say. If procedure comes to mind while you draft it, that is the signal to write the knowhow file first, see § "The most important rule".
4. **Should it notify, and on what?** Default is silent — `send_notification` only fires when there's something the user wants to hear about. Confirm whether a successful run should notify, and what the message should look like.
5. **Surface to review or stay silent?** Always ask unless the user's phrasing clearly answers it (see the table in "Where the thread lands"). `go_to_review: true` for "I want to read this when it finishes" (daily summaries, scheduled reports, alerts that need acknowledgement); omit for silent housekeeping. A `send_notification` doesn't answer this — notifications and review-surface are independent.
6. **If updating an existing trigger:** confirm which one — see "Edit, don't recreate" above.

Don't ask all six in one wall — pick the ones the user's request actually leaves open. A request like "every Monday at 9am summarize my open PRs and put it in front of me" already answers cron, intent, and review-surface — only confirm the notification shape if it's not obvious. A request like "say hello once in 2 minutes" answers cron and intent but **not** review-surface — confirm before creating.

## One-shot triggers

A one-off that just means "do this **now**" ("check X and tell me") should be handled inline — no trigger at all. But a one-off anchored to a **future time** ("remind me at 5pm today", "ping me in 20 minutes") is a real one-shot trigger: inline is impossible because you are not running at that time and nothing auto-resumes you, so an inline "reminder" is silently dropped. Create a one-shot trigger for any future-time one-off, and whenever the user explicitly asks for one (testing, demo, deliberate scheduling). A one-shot is just a normal trigger with a cron expression that matches a single upcoming moment; because it doesn't self-clean (below), the self-deleting variant is usually what you want.

**Leave `go_to_review` at its default (false / omitted)** so the single fire-thread goes straight to Archive instead of surfacing in the Current section. A one-shot reminder/test trigger's job is done the moment it fires — its thread isn't something the user needs to read afterward. This holds **even when the trigger sends a `send_notification` and/or deletes itself**: the notification is the user-facing output, and self-deletion is still the right outcome, but the thread itself stays in Archive. Only set `go_to_review: true` if the user explicitly wants to read the run afterwards.

A one-shot trigger does **not** self-clean. After firing, the cron expression no longer matches anything, but the trigger row stays in the trigger list — visible in pickers, the filter dropdown, and `list_triggers` output — until something deletes it. There are two acceptable ways to handle this; pick one with the user before creating:

1. **Leave it.** Tell the user it will sit in the trigger list after firing and they can delete it from the UI when they want. Don't promise to clean it up.
2. **Ask the trigger to delete itself.** Add a sentence in the user's voice to the intent — e.g. `"Send me a hello notification, then delete this trigger."` Keep it user-voice; don't name `delete_trigger` or paste in the trigger id. The engine wraps each trigger fire in an envelope that already tells the running LLM its own id and that self-deletion is permitted, so the intent doesn't need to repeat any of that. Then confirm to the user that the trigger will delete itself after firing.

Don't claim "I'll delete it after it runs" without doing one of the above — see "Promising behavior the trigger doesn't have" below.

## On-disk trigger definition (`trigger.toml`)

**The scheduler never reads this file.** `data/triggers/<slug>/trigger.toml` is a
**derived read-model** of the trigger's definition, mirroring the durable subset
of its config (`name`, `slug`, `schedule`, `timezone`, `run`, `on`, `app_id`,
`go_to_review`, `group_id`, `side_effect_grant`, `model`, `reasoning_effort`).
A trigger on the account chat defaults omits the last two. The engine maintains it from
the trigger events — written on create/update, removed on delete, fully rebuilt
from events on boot (ADR 0019). Runtime/identity fields (`id`, `last_run`,
`last_run_status`, `paused`) are deliberately omitted. It is **not
version-controlled**: the engine adds `data/triggers/*/trigger.toml` to the
workspace repo's local `.git/info/exclude`.

Events are authoritative — the scheduler runs off the event-replayed config, and
this file mirrors that config, never feeds it. Two rules follow:

- **Never hand-edit it.** The edit writes a file that reads correctly and changes
  nothing the scheduler sees; the next trigger event or restart overwrites it.
  Change triggers via `create_trigger`/`update_trigger` (or the UI), which emit
  the events the projection follows.
- **Never verify from it.** After a config change, re-read the trigger from
  `list_triggers` — that's the live registration. Reading `trigger.toml` back off
  disk proves only that you wrote it, so a change the scheduler never saw still
  verifies green.

Each firing is recorded as events (`TriggerExecuted` + `TriggerCompleted`, plus any
*domain event* the run emits), and the trigger's row in the triggers panel shows the
**last run's OK/failed status** next to its timestamp. There's no built-in run-history
view: for deeper detail on a threadless trigger's runs — what it found, when, why a run
failed — ask the *Lucidos Agent* (it reads the events via `query_events`) or build an
*app* on the trigger's events (`lucidos.events`), since every surface already reaches
them.

The file exists so a trigger is inspectable (the Plugins panel's installed-plugin
file links point at it for plugin-shipped triggers) and so a *plugin* can SHIP a
trigger by declaring one, see `plugins.md`.

### Renamed trigger → stale `run.path`

The folder is named by the trigger's `slug`, never by its current `name`.
Renaming moves nothing. Changing the slug (explicit `slug` via the CLI / HTTP
API, or a delete-and-recreate) relocates only `trigger.toml` to
`data/triggers/<new-slug>/` and deletes the old copy — `scripts/`, `knowhow/`,
and the registered `run.path` all stay under the old slug. The tell: a
`trigger.toml`-only folder beside a `scripts/`-only one.

Repair, in order:

1. `git mv` the old slug's `scripts/` and `knowhow/` (whichever exist) into
   `data/triggers/<new-slug>/`.
2. `update_trigger(trigger_id, run={type:"script", path:"triggers/<new-slug>/scripts/run.py"})`
   — only the event re-points the scheduler.
3. Confirm the new path in `list_triggers`, then delete the old folder. Deleting
   before step 2 lands removes the script the scheduler is still calling.

A broken run reports `Script not found: data/scripts/<path>` — the last candidate
in the path-resolution fallback, not the configured path. The configured one is
in `list_triggers`.

## Setup checklist

1. **Set timezone first** if not already set. Cron is 6 fields (`second minute hour day-of-month month day-of-week`) in the user's local timezone, DST-aware via IANA tz. The `create_trigger` tool refuses without a timezone. For anything beyond a plain daily or weekly time, read § "Writing cron expressions" above: the AND/OR split, the nth-weekday and last-weekday recipes, and the combinations the engine rejects.
2. **`list_triggers` first** to check whether an existing trigger should be updated instead of creating a new one.
3. **Decide cron vs. `on` (and whether `on` needs multiple entries)** before writing the trigger.
4. **Write the knowhow file, THEN `run.intent` as the user would say it.** The ordering is the rule, not a preference: see § "Write the knowhow file FIRST, then the intent" for the test and the worked example. Trigger-scoped recipes belong at `data/triggers/<slug>/knowhow/<descriptive>.md` — `<slug>` is fixed at creation (derived from the name when not given) and never re-derived, so after a rename the folder keeps the old name. The LLM tools take no `slug`; the CLI (`lucidos triggers create --slug`) and HTTP API do, and changing it strands `knowhow/` and `scripts/` under the old slug — see § "Renamed trigger → stale `run.path`". Broadly reusable recipes go in shared `data/knowhow/` (see `building-knowhow.md`). The trigger thread discovers knowhow the same way chat does — via `load_knowhow` calls the LLM makes itself — so there is no `run.knowhow` field to populate. Any legacy `run.knowhow:[...]` you might see in old `TriggerCreated` payloads is silently dropped by the deserializer; rewrite the intent to either name the relevant knowhow inline ("see `system-knowhow/X`") or be rich enough to nudge discovery from the system-prompt knowhow listing. Make the file's `name` and `description` frontmatter precise so semantic discovery finds it.

   Shared `data/knowhow/` is what you can write first. Trigger-scoped is the
   exception to the ordering: write that one *after* `create_trigger` returns,
   because only then is `<slug>` authoritative.

## Common mistakes to avoid

- **Recreating instead of editing.** See "Edit, don't recreate" above. The single biggest source of orphaned thread history.
- **Hand-editing `trigger.toml`.** It's a derived read-model the scheduler never reads: the edit silently no-ops (the trigger keeps its old config) and is clobbered by the next trigger event or restart. Change the config with `update_trigger`, then verify against `list_triggers` — never by reading the file back. See "On-disk trigger definition" above.
- **Resuming a paused trigger to "run it now", or hand-rolling the run.** Resume restores the schedule and runs nothing by itself. Use `triggers(action="run")` (or emit the subscribed event, for an event-only trigger) rather than copying the intent into `run_thread` or executing the script yourself. See "Running an existing trigger once, off-schedule" above.
- **Recipe-in-text.** Putting procedure into `run.intent` instead of knowhow. Almost always because the knowhow file was never written first. See "Write the knowhow file FIRST, then the intent" above.
- **A webhook condition without the `payload.` prefix.** The delivery is wrapped, so `{"action": "completed"}` matches nothing. Nothing warns you: the trigger stays healthy and never fires again. See § "The envelope: a webhook's body lands under `payload`".
- **Cron when a trigger subscription fits.** Polling burns runs and adds latency. If an event exists, prefer it.
- **Picking a trigger per event to maintain an aggregate without measuring first.** A trigger fire is a thread, not a callback, and a rollup's cost is usually dominated by the window it recomputes rather than by the rows that just arrived. Weigh it against a projection. See § "Aggregating events: cron, per event, or a projection".
- **Assuming day-of-month and day-of-week are ORed.** They are ANDed, so `0 0 9 1 * Mon` is "the 1st when it falls on a Monday", not "the 1st and every Monday". Vixie cron behaves the other way, which is where the assumption comes from. See § "Writing cron expressions".
- **A cron that can never fire.** `0 0 9 31 2 *` is valid syntax and a dead trigger. The engine rejects these now, but the surer habit is to read the next-3 fire times it reports on every create and update: they catch the whole class, including the expressions that fire far more rarely than the user meant.
- **Parallel triggers for one workflow that reacts to several events.** Use one trigger with multiple `on` entries; never duplicate the intent across siblings — editing one and forgetting the other silently drifts behaviour.
- **No knowhow file for a procedure the trigger clearly needs.** Without a discoverable knowhow file, the LLM re-derives the procedure every run and gets it slightly different each time. Write the recipe down — semantic discovery will surface it on the next fire.
- **Vague `name`/`description` frontmatter on a trigger-scoped knowhow.** Discovery is semantic, not by id, so a knowhow titled `notes.md` with `name: Notes` won't surface when the LLM is reasoning about an API call. Name the file by what it teaches (`openai-availability-check.md`), and write the `description` as the kind of question that should retrieve it.
- **Knowhow that recommends raw `curl`/`fetch` for an API the workspace already proxies.** When the recipe instructs the LLM to shell out with `curl -H "Authorization: Bearer $CRED_..."` (or the `requests`/`fetch` equivalent), the credential leaks into argv and tool transcripts. The right path is the `proxy_request` LLM tool against an entry in `data/config/apis.json` — see `system-knowhow/building-knowhow.md` § "Calling external APIs from a recipe".
- **Notifying on every tick.** A trigger that always notifies trains the user to ignore notifications.
- **Two live triggers with the same name.** Filter pickers and notification deep-links can't tell them apart. If you need two, name them differently.
- **Promising behavior the trigger doesn't have.** Only describe what's actually configured. Triggers do not self-clean — for a one-shot, follow the procedure in "One-shot triggers" above (either tell the user it will sit in the list, or ask the trigger to delete itself in the intent). Don't say "I'll delete it after it runs" unless you've actually wired that in.
- **Tool names or trigger ids in `run.intent`.** Intent is what the user would say, not how the LLM should act. Phrases like "call delete_trigger with trigger_id <uuid>" leak procedure into intent and re-paste runtime context (the trigger's own id) that the engine envelope already provides at fire time. Use user-voice ("then delete this trigger"); the runtime knows the rest.
