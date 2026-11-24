---
name: Triggers, Writing cron expressions
description: Part of Triggers. Its index lists the sections it covers.
---

<!-- domain-event: EmailReceived: Example domain event emitted by an email-ingestion integration. -->

One part of `system-knowhow/triggers`, which lists every part.

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
`crates/lucidos-agents-api/src/engine/tools/scheduler.rs` rewrites each plain
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

Swapping the weekday gives last Friday, last Tuesday and so on, with the same shape. Each has its own leap-February exception (for Friday it is 2036, 2064 and 2092).

For plain **month end** with no weekday, pin the last day per month-length class instead. Use `["0 0 9 31 1,3,5,7,8,10,12 *", "0 0 9 30 4,6,9,11 *", "0 0 9 28 2 *"]`. February is the awkward one either way, since its last day moves. `28` is a day early in leap years, and `28,29` fires twice in them. Pick one with the user.

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

### A fire due while the engine restarts

A scheduled fire never starts on an engine that is shutting down, because its run could not reach the API. The slot stays unrun, and the next engine's **missed-slot catch-up** fires it when it starts. A fire already waiting in the Thread Queue stays queued and runs on the next engine instead.

The catch-up fires only the most recent slot of the past hour, and only one with no recorded run at or after it. A recorded run counts whatever its status, `failed` included. So a run that failed on a healthy engine is not repeated on restart. Fix the cause, then run the trigger once if the user wants the result now.

The same catch-up runs when the computer wakes from sleep past a slot. In both cases a slot within the past 60 minutes fires once, however many slots the gap held.

### Skipped slots

A slot older than the 60-minute grace never runs. It becomes a **skipped slot**: Lucidos posts one inbox notice, with a push, per trigger per gap. The title reads "Morning brief: scheduled run skipped", or "Morning brief: 5 scheduled runs skipped". The body names the first and last slot in the trigger's timezone, and the tap opens the trigger.

A notice goes out only when the trigger did not run at all for the gap. A 15-minute poller that catches up once after a night asleep stays quiet. A 07:00 daily brief opened at 09:00 gets its notice.

These bound the gap, so their slots never count as skipped:

- the trigger's last run, its creation, and its latest pause, resume or edit;
- a snapshot restore;
- the gap an earlier notice already reported;
- a lookback of seven days, so a trigger off for a month reports one week at most.

The notice records a `TriggerSlotsSkipped` event, so a trigger can watch for it. To get the missed result, run the trigger once.

A run already in progress may finish before shutdown cuts it. On boot, an **interrupted run** gets a failed last-run status, one notification, and an inbox thread showing the retry decision. Boot reports cut runs from the past seven days. A working automatic resume after a user-initiated switch is excluded.

A qualifying interrupted run retries once through the Thread Queue in a **new thread**. It must have started less than 24 hours ago, before its next cron slot is due, with no newer fire of that trigger. It must not have completed, spawned sub-threads, or already been a retry. The trigger must still exist, be enabled and unpaused, and its original queued request must be recoverable. An already queued fire can cover the work instead.

The deadline is the earlier of 24 hours after fire start or its next cron slot. Admission at or after it drops the retry and notifies the user. A second interruption never causes a third run. An admitted fire that created no thread, including a script fire, requeues only once; a second cut drops it with a notification.

The retry header links back to the cut thread. Read that thread before repeating side effects: the interrupted run may already have sent a message or written a file. A trigger whose side effects must never repeat should say so in its intent. Ordinary failures on a healthy engine are not retried.

## `condition`: when to filter

Set `condition` on a trigger subscription when the event is high-volume and you care about only a slice. Example: subscribe to `EmailReceived` but fire only on emails from one sender. Without a condition, the trigger fires for every email and the run has to filter, which is slow and wasteful.

Conditions are pure payload filters, so never use one for logic that depends on external state ("only if this app's data file says X"). Stateful checks belong inside the run.

**One field is always available on a thread event: `thread_id`.** It is not in any event's payload: the engine supplies it from the thread the event belongs to. It scopes a subscription to a single thread, so `{ "event_type": "CodingAgentIdled", "condition": { "thread_id": "<uuid>" } }` fires only when THAT coding-agent session reaches a turn boundary. A **domain event** (one your workspace emits with `events(action="emit")`) belongs to no thread. It has no such field, so a `thread_id` condition on one matches nothing. Everything else a condition names is a **field path** into that event's own payload.

That payload includes the merged metadata fields `channel`, `actor` and `request_event_id` (`system-knowhow/thread-events/wire-format-and-metadata.md` § Wire format and metadata). So `{ "event_type": "ResponseGenerated", "condition": { "channel": "claude_code" } }` fires only when a coding-agent turn ends.

A persisted **system event** is the same case. `BackupFailed` belongs to no thread, so a `thread_id` condition never matches it. Condition on the variant's own fields instead, such as `filename` on `BackupCompleted`. The stored row wraps the event in a `type` / `data` envelope, and the matcher unwraps it for you, so never name those two keys.

**`ChangeProposed` also records parked work.** It fires for a new or updated change, including one the engine files straight into set-aside (`set_aside: true`). That covers work found on an archived thread's branch, and the first boot on a new version can file many at once. A trigger for "a change is ready to apply" therefore carries `{ "set_aside": { "$ne": true } }`. A pending proposal omits the field, so a bare `false` matches nothing.

### Who emitted a domain event: `actor`

**A domain event's `actor` is written by the engine, never by the emitter.** The engine records who called, and it drops any `actor` the emitter put in its own payload. So a condition on `actor.kind` reads who really emitted the event:

| Emitted by | `actor.kind` | Also carries |
|---|---|---|
| An app, through `lucidos.events.emit` | `device` | `device_id` only, never a name (see `thread-events.md`) |
| The `events(action="emit")` tool (the Lucidos Agent) | `agent` | `agent.kind` = `lucidos_agent` |
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

`delivery_payload` in `crates/lucidos-agents-api/src/api/webhooks.rs` builds the envelope. Two tests pin it: `a_senders_own_fields_land_under_payload` beside it, and `a_delivery_becomes_summary_headers_and_payload` in `crates/lucidos-e2e/tests/api_support/webhook_delivery_test.rs`, which reads the stored row back.

**Nothing warns you, and that makes this expensive.** A missing path and a present-but-null field both read as null, and the matcher cannot tell them apart. A subscription that can never match looks exactly like one patiently waiting. It arms clean, its panel row stays healthy, and `last_run` keeps the last real fire's timestamp.

**So diagnose by comparison, not by reading the panel.** Query the event store for the event type you subscribed to, and hold its newest row against the trigger's `last_run`. Deliveries arriving with no runs beside them is the tell.

**The general rule: write the condition against the STORED event, not the upstream payload you think you are subscribing to.** Only the shape of the row in the event store decides what a field path resolves to. So read one real stored event first, with the `events` tool's `query` action and `limit` 1, then write every path from what you see. A path copied out of the sender's API docs is a guess.

**Script triggers inherit the same envelope.** `TRIGGER_EVENT_PAYLOAD` holds the whole event payload, wrapper included, so a script has to reach through `payload` too. A hand-emitted test event is usually written flat, so a script can pass your test and still fail on the real delivery. Read it defensively:

```python
raw = json.loads(os.environ.get("TRIGGER_EVENT_PAYLOAD", "{}"))
body = raw.get("payload") if isinstance(raw.get("payload"), dict) else raw
```

## Continue key

By default every fire starts a new thread. Set `continue_key` on an intent trigger when later fires about the same thing belong in the thread an earlier fire started: comments on one ticket, replies in one chat thread, updates to one pull request. The key ties them to one *root thread*.

A key spec is a list of parts. Each part is a **field path** in the syntax a `condition` uses. A part can also list field paths, and the first one with a value wins. Lucidos resolves every part against the fire's stored event payload, so the § "The envelope" rules apply: a webhook's fields sit under `payload.`.

| Spec | Key for one fire |
|---|---|
| `["payload.repository.full_name", ["payload.issue.number", "payload.pull_request.number"]]` | `["example-org/example-repo","42"]`, for an issue or a pull request alike |
| `["channel", "thread_ts"]` | `["C123","1700000000.1234"]`, on a domain event your translation script emits |
| `["ticket_id"]` | `["T-17"]` |

How a keyed fire lands:

- **The key is the JSON array of the resolved values.** Lucidos compares it and never parses it. A number and the same digits as a string give the same key.
- **Every part must resolve** to a non-empty string, number or boolean. If any part has no value, the fire has no key and starts a new, unkeyed thread.
- **A key that matches an open thread continues it.** The fire arrives there as a new message, and the thread's first message still holds the intent. Otherwise a new thread starts and keeps the key.
- **Keys are shared across triggers.** Two triggers that build the same key reach the same thread. That lets an intake trigger and a follow-up trigger feed one root thread. Put a source name in the spec when two sources could collide.
- **One key maps to at most one open thread**, even when two fires race.
- **The Archive action releases the key.** The next fire with that key starts a fresh thread, and unarchiving does not take the key back. An unattended run that lands in Archive on its own when it finishes keeps its key, so the next comment still reaches it.
- **A retry starts its own unkeyed thread**, because the run it retries still holds the key.

**A keyed fire can answer a question.** When the thread waits on a question card, the fire answers it as free text, exactly like a typed answer. The agent judges whether the text really answers the question, and asks again if not. Use the trigger's `condition` to decide who may answer from the source.

Only intent triggers take a key: a script fire starts no thread to continue. `continue_key: null` or `[]` clears a spec. The key is only for input. Replies to the source go out through workspace script triggers on thread events, which read the address from the thread's first delivery.

