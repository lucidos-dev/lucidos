---
name: Lucidos CLI (lucidos), Subcommands (continued)
description: Part of Lucidos CLI (lucidos). Its index lists the sections it covers.
---

<!-- domain-event: OuraSleepImported: Example domain event emitted by a wearable sleep-data import. -->

One part of `system-knowhow/lucidos-cli`, which lists every part.

## Subcommands (continued)

### `lucidos request-read yes|no`

Record this turn's *read decision*: whether the user should read **this
thread's** latest reply. Every turn ends with one, so run it before your final
message. The answer is required, and the bare form is refused. It is the
coding-agent counterpart of the chat agent's `request_read` tool, and records
the same events: `ThreadReadRequested` for `yes`, `ThreadReadNotRequested` for
`no` (ADR 0409, ADR 0417). It takes the thread from `$LUCIDOS_THREAD_ID` and
has no thread flag, so it can only decide for your own reply.

Say `yes` when you end with an answer the user asked for and no change to
apply: an investigation, a review, a researched answer. Say it too when they
said they would read it later. Once your turn ends, the thread waits under
**Review** until they have seen your final reply. Say `no` for a plain "done",
and when you propose a change, since a ready change already lists the thread
under Review. Say `no` too for a progress report while your sub-threads still
run, your event wait is armed, or a successor session carries the work on.
Say `yes` then only for something the
user should read before the work finishes, such as a failure or a finding.

A sub-thread whose parent acts on its report, a `wake` or `wake-when-all-done`
child, says `no` for a routine result such as a review verdict. The user reads
the parent, which asks them for any approval. Say
`yes` only for a failure, a gap the user must know about, or something only the
user can do or decide. A child with no declared wake mode decides its own wake,
and follows the same rule for a turn that wakes its parent. A turn the user
opens in your thread is answered as usual (ADR 0607).

```bash
$ lucidos request-read yes
{"status":"requested","message":"Recorded. Once this turn ends, the thread is listed under Review until the user has read your reply. A ready change or a harden hold that lists it already takes its place."}
$ lucidos request-read no
{"status":"not-requested","message":"Recorded. The thread does not wait for the user to read this reply."}
```

A turn that ends without deciding is not left undecided. Lucidos asks a model
once, from your turn's message and final reply, and records its answer. When a
ready change or a harden hold already lists the thread, it records `no` without
asking.

It does not notify. Run `lucidos notify` as well only when the user should
hear now. A request clears once the end of your reply has been on screen for
a second. The user's next message or an archive also clears it. A later `no`
never clears it.

### `lucidos expect-wake <duration> --reason <R>`

State this thread's *expected wake*: when it will next wake, from the event
waits and child threads you leave pending. Run it before you finish a turn that
leaves any pending, for the earliest of them. The duration takes a unit:
`90s`, `45m`, `2h` or `1h30m`. A bare number is refused. The reason says what
you wait for and why it should come by then.

It is the coding-agent counterpart of the chat agent's `expect_wake` tool, and
records the same `WakeExpected` (ADR 0603). It takes the thread from
`$LUCIDOS_THREAD_ID` and has no thread flag. The span runs from one second to
24 hours, the longest event wait.

```bash
$ lucidos expect-wake 45m --reason "the release build, about 40 minutes per run"
{"status":"recorded","message":"Recorded: you expect to wake in about 45 min. While the keep-alive is on, it keeps the prompt cache warm until then when that costs clearly less than one reread. A later statement replaces this one."}
```

Estimate from what you wait for. With the keep-alive on, `lucidos await-event`
on a keep-warm event adds how long past waits like it lasted in this workspace,
and asks for this statement. The newest statement stands, so run it again when
you learn more. Nothing needs it while the keep-alive is off.

### `lucidos wake-parent yes|no`

Record this turn's *wake decision*, for a sub-thread whose parent declared no
*wake mode*: whether your report wakes your parent thread. Run it before your
final message. The answer is required, and the bare form is refused. It is the coding-agent counterpart of the chat
agent's `wake_parent` tool, and records the same events:
`ParentWakeRequested` for `yes`, `ParentWakeNotRequested` for `no`.

It takes the
thread from `$LUCIDOS_THREAD_ID` and has no thread flag. A thread with no parent
gets a 409. A child whose parent declared its wake mode records nothing: the
answer has status `declared`, and its report follows the mode.

Say `yes` when your parent must act on the report now: it waits for this result
to take a next step, combine results, retry, or decide something only it can.
Say `no` when the report only informs. The user reads your findings in your own
thread, and your parent reads the report on its next turn.

An integrated sub-thread reports ordinary decisions and options to its
parent. Run `lucidos wake-parent yes` before ending the turn with that report.
The spawn's wake mode still controls whether the report wakes the parent.

```bash
$ lucidos wake-parent no
{"status":"file","message":"Recorded. When this turn ends cleanly, your report is filed on your parent thread, which reads it on its next turn. A failure still wakes it, and a follow-up that reaches you first reopens the decision."}
```

A `no` binds only a clean finish. A failure, a cancel or a turn an engine
restart cut short always wakes the parent. A turn that ends without deciding is
decided by one model call, and any doubt wakes the parent. A decision binds one
report: your next turn starts undecided, and so does a follow-up that reaches
you mid-turn.

### `lucidos notify --title <T> --message <M> [--app-id <APP>] [--tap <T>] [--thread-id <UUID>] [--event-id <UUID>] [--fragment <FRAG>]`

Send a push notification via the parent workspace. It persists to the inbox AND fans out as a web push to subscribed devices, exactly like a `send_notification` LLM tool call. Any subprocess (Python script, bash script, scheduled `script:`-typed trigger) can call it without an LLM thread.

```bash
$ lucidos notify --title "Nightly backup done" --message "Backup completed: 1,240 rows archived"
{"success":true,"notification_id":"5b1e..."}
```

`--title` and `--message` are both required and non-empty (the engine returns 400 on an empty value).

`--app-id <id>` is optional and stamps the notification's deep-link target. **Set it only when tapping should open that app to act on it.** Most reminders, nudges and summaries do not deep-link, even when their trigger lives inside an app dir. The `send_notification` LLM tool follows the same rule.

#### Deep-linking back to the originating event

For event-driven triggers ("coding agent is asking", "credential needed", …) the tap should land on the exact card the user must act on. Four flags wire this up:

- **`--tap <modal|navigate>`**: which kind of tap. `modal` (default) opens the inbox detail. Use it for purely informational pushes too ("Backup complete", "Sync finished"): every notification is openable, and there is no passive kind (the old `none` kind is retired). `navigate` deep-links to the target the other flags name: `--thread-id` opens that thread (scrolling to and pulsing `--event-id` when set), and `--app-id` opens that app. With both, the thread wins, since "answer this question" is the common shape.
- **`--thread-id <UUID>`**: the originating thread. With `--tap navigate`, the tap opens this thread instead of the inbox modal. Even without `--tap`, it stamps the notification so the modal's "Open thread" button resolves.
- **`--event-id <UUID>`**: a specific event id inside `--thread-id` to scroll to and briefly pulse when the tap lands. Ignored when `--thread-id` is absent. An event that is not in that thread fails the command with a 400, and so does a domain event, which lives in no thread.
- **`--fragment <string>`**: the place INSIDE the app the tap lands on. It arrives as the app's `location.hash`, so an app that routes on the hash opens on that item. Only read on the `--app-id` branch, since a thread deep-link names no app. An app that ignores the hash still opens.

```bash
# Deep-link the push to the exact UserQuestionAsked card on tap.
lucidos notify \
  --title "Coding agent is asking" \
  --message "Ship it?" \
  --tap navigate \
  --thread-id "$TRIGGER_EVENT_THREAD_ID" \
  --event-id "$TRIGGER_EVENT_ID"
```

The engine sets `TRIGGER_EVENT_THREAD_ID` and `TRIGGER_EVENT_ID` on every script trigger fired by a thread-scoped event (see `system-knowhow/triggers/actor.md` § "Script trigger env vars"). A schedule-fired trigger gets neither, so `--tap modal` (the default) is the only meaningful choice.

An app deep-link takes `--fragment` the same way, so the tap lands on the item:

```bash
# Land on the habit the reminder is about, not on the board's default sort.
lucidos notify \
  --title "A streak is at risk" \
  --message "Hydration has no entry for today." \
  --tap navigate \
  --app-id habit-tracker \
  --fragment "habit-hydration"
```

The CLI rejects `--tap navigate` with neither `--thread-id` nor `--app-id` (it needs a destination) before the HTTP round-trip. The server returns the same 400 if that check is bypassed. Panel-shaped targets (`changes`, `triggers`, `files`, …) have no CLI flag: use the `send_notification` LLM tool, or POST to `/api/v1/notifications` with the full structured `tap` object.

#### Response and exit codes

The CLI prints the engine's JSON response on stdout (`{"success": true, "notification_id": "<uuid>"}`). Non-zero exit on transport / HTTP error, with the engine's error body (or `lucidos: <transport error>`) on stderr.

#### When to use which

| Context | Use |
|---|---|
| Scheduled `script:`-typed trigger that needs to nudge the user | `lucidos notify` |
| One-off bash / Python script run as part of an app or trigger | `lucidos notify` |
| LLM agent in a chat / trigger thread | `send_notification` tool (LLM picks `app_id` based on context) |
| Background engine code (Rust) | `LucidosEngine::create_notification` (the shared helper both surfaces call) |

### `lucidos notifications list | read --id <uuid> | read-all`

Read and clear the notification *inbox* (`notify` only *sends*). It is generated
from the capability parity manifest, so it uses the same gateway-safe HTTP
client as every other subcommand. Use it instead of hand-rolled `curl`, which
would have to guess the engine port and the gateway `/<slug>/` path prefix.

```bash
# What's unread? (default filter is unread; pass --filter all for everything)
$ lucidos notifications list
[ { "id": "c3dac86b-…", "title": "Backup failed", "message": "…", "read": false, "created_at": "…" } ]

# Clear one by id (from the list above)
$ lucidos notifications read --id c3dac86b-bfd1-4f1e-a9d2-b47567957d25

# Clear the whole unread inbox
$ lucidos notifications read-all
```

`list` accepts `--filter unread|all` and `--limit N` (1–50, default 20). `read`
requires `--id <uuid>`. Both `read`/`read-all` emit `NotificationRead` /
`NotificationsAllRead` so other devices' unread state syncs over SSE. Exit
non-zero on transport / HTTP error.

A SQL write to `notification_reads` is refused. It emitted nothing, so every
unread count stayed wrong until a reload.

> **In-thread agent:** the chat Lucidos Agent has the grouped `notifications`
> tool (`action: list | mark_read | mark_all_read`), which runs **in-process**.
> Use the tool from a chat or trigger thread. Use this CLI from a
> `script:`-typed trigger or a coding-agent, bash or Python subprocess. Both are
> checked against the same capability parity manifest, so they cannot drift.

### `lucidos preferences get | set --key <K> --value <V> | reset --key <K>`

Read and change user *preferences* (Settings). Generated from the capability
parity manifest (gateway-safe HTTP client). `get` lists every settable key with
its current value, allowed values, default, and scope; `set` changes one.
`reset` deletes a global key's stored value, so the key reads its default.
A reset of `timezone` or `language` takes effect at once, as a `set` does.

```bash
$ lucidos preferences get
$ lucidos preferences set --key timezone --value Europe/Oslo
$ lucidos preferences set --key chat_model --value claude-opus-5
$ lucidos preferences reset --key external_link_target
```

`set` and `reset` both emit `PreferencesChanged`, so every open page follows
the change. A SQL write to `preferences` is refused (`system-knowhow/best-practices/workspace-layout.md` § The
workspace database). `reset` reaches global keys only. Put a per-device key
back with `set` and its default value.

`get` accepts `--device-id <id>` (read device-scoped overrides; omit for the
global view). `set` requires `--key` + `--value`; pass `--device-id` only for a
per-device key. The chat agent's in-process equivalent is the grouped
`preferences` tool (`action: get | set`).

Run from an agent's own process, `set` and `reset` take only the keys the
agent's tool may write. A key the catalog keeps for the owner, such as `command_guard` or
`network_bind`, answers 403 (`system-knowhow/preferences/how-a-write-propagates.md` § Who may write a key).

### `lucidos triggers list | create | update | delete | run`

Manage *triggers*: scheduled (cron) and/or event-driven automations. Generated
from the manifest. The rich fields (`run`, `on`, `cron_expressions`,
`side_effect_grant`, `mcp_servers`, `continue_key`) are passed as JSON strings.
`actor` is a plain payload path string. Run from an agent's own process, a
`--side-effect-grant` or `--coding-agent-grant` that widens the trigger's grants
answers with a refusal: only the owner grants a trigger.

```bash
$ lucidos triggers list
# Create a daily 8am intent trigger (cron in the user's local timezone)
$ lucidos triggers create --name "Morning digest" \
    --run '{"type":"intent","intent":"summarise overnight emails"}' \
    --cron-expressions '["0 0 8 * * *"]'
# Event-driven trigger with a payload filter
$ lucidos triggers create --name "Bad sleep alert" \
    --run '{"type":"intent","intent":"nudge me to rest"}' \
    --on '[{"event_type":"OuraSleepImported","condition":{"sleep_score":{"$lt":70}}}]'
# Update keeps run history (prefer over delete+create); pause/resume via --paused
$ lucidos triggers update --id <uuid> --paused true
$ lucidos triggers delete --id <uuid>
# Fire an existing trigger once, right now, outside its schedule
$ lucidos triggers run --id <uuid>
# Pin an intent trigger to its own model and thinking budget
$ lucidos triggers update --id <uuid> --model gemini-3.8-flash --reasoning-effort low
# Pin which backend serves a model that has more than one route
$ lucidos triggers update --id <uuid> --model claude-opus-5-5 --provider anthropic
# Unpin all three, back to the account default
$ lucidos triggers update --id <uuid> --model null --reasoning-effort null --provider null
```

With `create` or `update`, use `--folder-grants '["/opt/example"]'` and
`--program-grants '["example"]'` to request grants as part of the write.
Lucidos puts the additions on one permission card in the calling thread.
The owner approves the listed access before it joins the trigger's settings.

On `--model`, `--reasoning-effort` and `--provider`, the value `null` clears
that pin. A pin flag left out keeps the pin as it is. Other flags store `null`
as a literal string.

`--provider` needs a model pin, from the same request or already on the
trigger, and must name one of that model's routes. Anything else is refused at
save time. A later `--model` change clears the pin unless the same request sets
`--provider` again. At fire time, a pin to a backend with no credential refuses
the fire rather than running it elsewhere.

`--cron-expressions` entries are validated on `create` and `update`. Within one
expression the fields are ANDed and across the array they are ORed, so
`0 0 9 1 * Mon` is the 1st only when it is a Monday. An expression that can
**never** fire (`0 0 9 31 2 *`, Feb 31) is refused with an error naming the
offending fields. A successful write returns a `cron_preview` object with
`next_runs` (the next few fire times) plus any `warnings`. Read the preview
back rather than assuming the schedule means what you intended.
`system-knowhow/triggers/writing-cron-expressions.md` § "Writing cron expressions" has the recipes.

`create`/`update` accept `--name`, `--run`, `--cron-expressions`, `--on`,
`--app-id`, `--go-to-review`, `--group-id`, `--side-effect-grant`,
`--coding-agent-grant`, `--mcp-servers`, `--slug`, `--model`,
`--reasoning-effort`, `--provider`; `update`/`delete`/`run` take `--id <uuid>`.
`--coding-agent-grant true` is refused from inside a Lucidos thread, since only
the user grants it (`system-knowhow/triggers/grouping-triggers.md` § "Coding-agent grant"). `false` always
passes. The chat agent's in-process
equivalent is the grouped `triggers` tool (`action: create | list | update |
delete | pause | resume | run`). Pause/resume are tool-only there; the CLI
pauses via `update --paused`.

`run` performs an **off-schedule run**: a real fire that records
`TriggerExecuted` / `last_run` and carries the trigger's own identity,
program grants and `go_to_review`, indistinguishable downstream from a
scheduled one. It returns as soon as the run is admitted, so a `success: true`
response does not mean the work finished. Read `status` in the response body:
`started`, `queued` (over capacity), or `already-running` (a fire was already
active or queued, so **nothing new started**). It is refused for a paused
trigger and for an event-only one (emit its subscribed event with
`lucidos events emit` instead); the `message` field says which.

### `lucidos trigger-groups list | create | rename | reorder | delete`

Manage *trigger groups*: the folders that organize triggers in the panel. A
group is only a label and fires nothing.

```bash
$ lucidos trigger-groups list
$ lucidos trigger-groups create --name "Health" --order 10
$ lucidos trigger-groups rename --id <uuid> --name "Wellbeing"
$ lucidos trigger-groups reorder --ordering '[{"id":"<uuid>","order":0}]'
$ lucidos trigger-groups delete --id <uuid>
```

Assign a trigger to a group with `lucidos triggers update --id <uuid>
--group-id <group-uuid>`. The chat agent's in-process equivalent is the grouped
`trigger_groups` tool.

### `lucidos apps list | get --id <id> | update | delete`

List, inspect, rename, or delete *apps*. Creating an app and editing its source
are not CLI ops: creation is the chat agent's `create_app` tool, and source
editing happens in the app's coding-agent worktree.

```bash
$ lucidos apps list
$ lucidos apps get --id habit-tracker
$ lucidos apps update --id habit-tracker --name "Habit Tracker" --description "Daily habits"
$ lucidos apps delete --id habit-tracker
```

`get`/`update`/`delete` take `--id`; `update` takes `--name` (required) +
`--description`. Plugin-installed apps refuse `delete` (remove the plugin
instead). `list`/`get` are also in the JS SDK (`lucidos.apps`); `update`/`delete`
are CLI-only.

### `lucidos thread-queue list | run-now --entry-id <uuid> | drop --entry-id <uuid>`

Inspect the *Thread Queue* (background admission control). `list` prints the
live queue and the active *capacity policy* as JSON. `run-now` force-admits a
queued entry, ignoring caps. `drop` removes a queued entry without running it.

```bash
$ lucidos thread-queue list
$ lucidos thread-queue run-now --entry-id 0b1e…  # force-admit
$ lucidos thread-queue drop --entry-id 0b1e…     # cancel a queued entry
```

Get an entry id from `list` (`entries[].id`). Mirrors the chat agent's grouped
`thread_queue` tool. Changing the capacity policy is that tool's
`update_policy` action, and deliberately **not** a CLI command. The raw
`PUT /api/v1/thread-queue/policy` replaces omitted caps with defaults, while the
LLM tool merges with the live policy.

