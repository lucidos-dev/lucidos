---
name: Building a Trigger
description: Use when the user wants something to happen automatically — "every morning", "notify me when X happens", "watch for Y", recurring or event-driven background work. Covers cron vs event subscriptions, the intent-vs-procedure rule, and notification discipline.
---

# Building a Trigger

How to guide a user from "I want this to happen automatically" to a working trigger. Cron format, frontmatter, and field reference live in the engine system prompt and the grouped `triggers` tool description — don't restate them. The CLAUDE.md "trigger intent vs. procedure" rule is summarized below; see `docs/taxonomy.md` § Triggers for the worked example.

> **Tool surface.** Triggers are managed through the grouped **`triggers`** tool
> (`action: create | list | update | delete | pause | resume`) and the grouped
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

If the user only wants it to happen **once, right now** — a check, a lookup, a computation — just do it inline; no trigger. But if the one-off is anchored to a **future time** ("remind me at 5pm", "ping me in 20 minutes"), it is NOT an inline task: you are not running at 5pm and nothing auto-resumes you, so an inline "reminder" is silently dropped. A future-time one-off needs a **one-shot trigger** (ideally self-deleting) — see "One-shot triggers" below.

## The most important rule: `run.intent` is intent, not procedure

A trigger's `run.intent` is **what the user would say** — one sentence in their voice. Everything about *how* (which API to hit, how to parse, what to retry, when to fall back) belongs in a knowhow file. The trigger thread looks up knowhow itself by calling `load_knowhow` at fire time — same as a chat session — so the trigger config has no per-trigger allow-list to configure. The HTTP API will accept a procedure-laden intent text and won't stop you — see `docs/taxonomy.md` § Triggers for the worked bad/good example.

### Don't paste procedure into `run.intent` to "make sure" the LLM sees it

The trigger thread inherits the same knowhow surface a chat thread has: the system prompt's intent registry advertises what's available, and the LLM calls `load_knowhow` when it judges a recipe relevant. Writing the procedure inline to bypass that lookup turns the intent into a recipe and the next person who reads the trigger config can't tell what the user originally asked for. Keep the intent in the user's voice ("send me a daily summary of open PRs") and let the LLM pull the procedure on demand.

## Cron vs. `on` vs. both

- **Cron** — "every morning at 8" / "weekdays at noon". Time-driven.
- **`on`** — "when X happens". Reactive. Each entry in the `on` array names an event type plus an optional payload filter. The event must already be emitted by something (an app, another trigger, an integration).
- **Both** — rare; usually means cron with a payload-shaped condition that should be event-driven instead. Re-examine before doing this.

If the user says "notify me when X" and X isn't an event yet, you have two work items: (1) make X emit an event, (2) trigger on it. Tell the user that explicitly.

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

## `condition` — when to filter

Set `condition` on a subscription when the event is high-volume and you only care about a slice. Example: subscribe to `EmailReceived` but only fire on emails from a specific sender. Without a condition, the trigger fires for every email and the LLM has to filter inside the run — wasteful and slow.

Don't use `condition` for logic that depends on external state (e.g. "only if this app's data file says X"). Conditions are pure payload filters. Stateful checks belong inside the run.

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

#### Worked example — push when agent needs me

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

### Script trigger env vars

When the engine fires a script trigger that subscribes to a domain event, it sets the following env vars before exec'ing the script. Schedule fires emit none of them — the script has no source event to point at.

| Env var | Set when | What it holds |
|---|---|---|
| `TRIGGER_EVENT_TYPE` | Always on event fires | The matched event name (e.g. `UserQuestionAsked`). Use as a fallback title or when the script genuinely needs to branch on type. |
| `TRIGGER_EVENT_PAYLOAD` | Always on event fires | The source event's payload, serialized as JSON. Parse with `json.loads(os.environ["TRIGGER_EVENT_PAYLOAD"])`. |
| `TRIGGER_EVENT_ID` | When the source event has a row id | The `events.id` (UUID) of the source row. Pass to `lucidos notify --event-id` so the push tap scroll-and-pulses the exact card. |
| `TRIGGER_EVENT_THREAD_ID` | Only for *thread-scoped* source events | The thread the source event lives on. Pass to `lucidos notify --tap navigate --thread-id` so the push deep-links to the originating conversation instead of the trigger's own thread (which is `LUCIDOS_THREAD_ID`). |

The trigger's own thread is `LUCIDOS_THREAD_ID` (same env var every spawned subprocess gets). `TRIGGER_EVENT_THREAD_ID` is the *source* event's thread — these are different threads. A script that mixes them up will deep-link the push into the trigger's own (uninteresting) thread instead of where the user actually needs to act.

### Worked example — push when any subscribed event fires

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

This matters **only when the workspace has the command guard on** (Settings → Permissions → Command Safety; off by default). When it's on, the command guard classifies every `run_bash` / `run_python` command a trigger's intent runs. Most commands (reads, data crunching, downloads, writes inside the workspace) run untouched. But an **irreversible** one — sending email, a mutating HTTP request (POST/PUT/DELETE), a cloud-CLI change (`gh`/`aws`/`gcloud`), destroying files outside the workspace — is gated.

A chat turn would *ask* the user to approve such a command. A trigger fires unattended: there's nobody to ask. So instead the trigger carries a **side-effect grant** — the set of irreversible side-effect categories it's pre-authorized to perform. At fire time:

- the command's side-effect category **is in the grant** → it runs;
- it **isn't** → the command is blocked and **the whole trigger run fails** (a failure notification surfaces it, naming the missing grant).

The categories are: **email**, **external API** (mutating HTTP), **cloud CLI** (gh/aws/gcloud), **out-of-workspace destruction**, and **other** (anything irreversible that fits none of the above). The default grant is empty — a new trigger may perform *no* irreversible side-effect.

**The grant is set by the user, not by you.** The `create_trigger` / `update_trigger` tools do **not** accept a grant field — that's deliberate, so an autonomous agent can't widen its own unattended authority. The user grants side-effects in the trigger's settings UI (the "Allowed side-effects" checkboxes). So when you build a trigger whose intent needs an irreversible side-effect (e.g. "email me the digest every morning"), **tell the user** they must tick the matching side-effect (here, *Send email or messages*) in the trigger's settings, or — if Command Safety is on — the run will fail the first time it tries to send. If Command Safety is off, none of this applies and the command runs unguarded.

**The grant also flows to coding-agent work the trigger spawns.** When a trigger's intent launches a *coding-agent thread* (Claude Code / Codex) — directly, or via a sub-thread an orchestrator spawns — that thread runs **unattended**, with no human to answer the coding agent's permission cards. Instead of hanging on a card forever, the engine resolves each request from the same side-effect grant (it walks the spawn tree to its root trigger and inherits that trigger's grant): benign in-workspace work (reads, in-workspace edits, git, `lucidos data write` to `data/`) is auto-allowed, an irreversible side-effect is allowed only if its category is in the grant, and a catastrophic command is always denied. Unlike the chat command guard (which fails the *whole* run on an ungranted side-effect), the coding-agent path denies just the one request — the agent gets the denial and works around it or reports the step failed. This is independent of the Command Safety toggle. So a coding-agent trigger that needs, say, a mutating HTTP call still needs the user to tick **Call external APIs** on the trigger; otherwise that one call is denied (the rest of the run proceeds). See `coding-agent-events.md` § "Unattended auto-resolution".

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

## Questions to settle with the user before creating

Don't call `create_trigger` from the user's first message. Most "create a trigger for X" requests leave at least one of these unsettled — confirm before writing the trigger. Skip questions only when the user has already answered them in the same turn.

1. **Recurring or one-shot — and if one-shot, now or at a future time?** Triggers are for things that should keep happening, so a recurring need is always a trigger. A one-off splits by *when*: if it's "do this **now**" ("check X and tell me"), handle it inline — no trigger. If it's anchored to a **future time** ("remind me at 5pm today", "ping me in 20 minutes"), it CANNOT be handled inline — you are not running then and nothing auto-resumes you, so an inline reminder is silently dropped — so it needs a **one-shot trigger** (cron for that time, ideally self-deleting). Whenever you create a one-shot (a future reminder, or an explicit test like "fire once in 2 min"), ask whether it should delete itself after firing — it won't on its own. Create it with `go_to_review` omitted (so the fire-thread lands in Archive, not the Current section) unless the user explicitly wants to read the run afterwards. See "One-shot triggers" below for the procedure.
2. **Cron or `on`?** "Every morning at 8" is cron. "When my package ships" is an event subscription. If the user names several events the same workflow should react to ("when X *or* Y happens"), they belong in one trigger with multiple `on` entries — not parallel triggers. If the event doesn't exist yet, name the work (emit the event from somewhere, then trigger on it) and confirm.
3. **What's the run.intent in the user's voice?** One sentence the user would actually say. If you're tempted to write the procedure here, stop and put it in knowhow instead.
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

Every trigger has a **derived read-model** of its definition at
`data/triggers/<slug>/trigger.toml`, mirroring the durable subset of its config
(`name`, `slug`, `schedule`, `timezone`, `run`, `on`, `app_id`, `go_to_review`,
`group_id`, `side_effect_grant`). It's maintained by the engine from the trigger
events — written on create/update, removed on delete, and fully rebuilt from
events on boot (ADR 0019).

It is **NOT the source of truth and NOT version-controlled**: events are
authoritative (the scheduler runs off the event-replayed config, never the file),
and the engine adds `data/triggers/*/trigger.toml` to the workspace repo's local
`.git/info/exclude`. **Don't hand-edit it** — a change is overwritten on the next
trigger event or restart; edit triggers via `create_trigger`/`update_trigger`
(or the UI), which emit the events the projection follows. Runtime/identity
fields (`id`, `last_run`, `last_run_status`, `paused`) are deliberately omitted.

Each firing is recorded as events (`TriggerExecuted` + `TriggerCompleted`, plus any
*domain event* the run emits), and the trigger's row in the triggers panel shows the
**last run's OK/failed status** next to its timestamp. There's no built-in run-history
view: for deeper detail on a threadless trigger's runs — what it found, when, why a run
failed — ask the *Lucidos Agent* (it reads the events via `query_events`) or build an
*app* on the trigger's events (`lucidos.events`), since every surface already reaches
them.

The file exists so a trigger is inspectable (the Plugins panel's installed-plugin
file links point at it for plugin-shipped triggers) and so a *plugin* can SHIP a
trigger by declaring one — see `building-a-plugin.md`.

## Setup checklist

1. **Set timezone first** if not already set. Cron is 6 fields (`second minute hour day-of-month month day-of-week`) in the user's local timezone, DST-aware via IANA tz. The `create_trigger` tool refuses without a timezone.
2. **`list_triggers` first** to check whether an existing trigger should be updated instead of creating a new one.
3. **Decide cron vs. `on` (and whether `on` needs multiple entries)** before writing the trigger.
4. **Write `run.intent` as the user would say it.**
5. **If the trigger needs a procedure-laden recipe, write it to a knowhow file.** Trigger-scoped recipes belong at `data/triggers/<slug>/knowhow/<descriptive>.md` (where `<slug>` is the trigger's kebab-case slug field — set it explicitly via `create_trigger`/`update_trigger`; if you don't, the engine derives one from the name on read but never persists it, so renaming the trigger silently moves the per-trigger knowhow path). Broadly reusable recipes go in shared `data/knowhow/` (see `building-knowhow.md`). The trigger thread discovers knowhow the same way chat does — via `load_knowhow` calls the LLM makes itself — so there is no `run.knowhow` field to populate. Any legacy `run.knowhow:[...]` you might see in old `TriggerCreated` payloads is silently dropped by the deserializer; rewrite the intent to either name the relevant knowhow inline ("see `system-knowhow/X`") or be rich enough to nudge discovery from the system-prompt knowhow listing. Make the file's `name` and `description` frontmatter precise so semantic discovery finds it.

## Common mistakes to avoid

- **Recreating instead of editing.** See "Edit, don't recreate" above. The single biggest source of orphaned thread history.
- **Recipe-in-text.** Putting procedure into `run.intent` instead of knowhow. See "The most important rule" above.
- **Cron when an event subscription fits.** Polling burns runs and adds latency. If an event exists, prefer it.
- **Parallel triggers for one workflow that reacts to several events.** Use one trigger with multiple `on` entries; never duplicate the intent across siblings — editing one and forgetting the other silently drifts behaviour.
- **No knowhow file for a procedure the trigger clearly needs.** Without a discoverable knowhow file, the LLM re-derives the procedure every run and gets it slightly different each time. Write the recipe down — semantic discovery will surface it on the next fire.
- **Vague `name`/`description` frontmatter on a trigger-scoped knowhow.** Discovery is semantic, not by id, so a knowhow titled `notes.md` with `name: Notes` won't surface when the LLM is reasoning about an API call. Name the file by what it teaches (`openai-availability-check.md`), and write the `description` as the kind of question that should retrieve it.
- **Knowhow that recommends raw `curl`/`fetch` for an API the workspace already proxies.** When the recipe instructs the LLM to shell out with `curl -H "Authorization: Bearer $CRED_..."` (or the `requests`/`fetch` equivalent), the credential leaks into argv and tool transcripts. The right path is the `proxy_request` LLM tool against an entry in `data/config/apis.json` — see `system-knowhow/building-knowhow.md` § "Calling external APIs from a recipe".
- **Notifying on every tick.** A trigger that always notifies trains the user to ignore notifications.
- **Two live triggers with the same name.** Filter pickers and notification deep-links can't tell them apart. If you need two, name them differently.
- **Promising behavior the trigger doesn't have.** Only describe what's actually configured. Triggers do not self-clean — for a one-shot, follow the procedure in "One-shot triggers" above (either tell the user it will sit in the list, or ask the trigger to delete itself in the intent). Don't say "I'll delete it after it runs" unless you've actually wired that in.
- **Tool names or trigger ids in `run.intent`.** Intent is what the user would say, not how the LLM should act. Phrases like "call delete_trigger with trigger_id <uuid>" leak procedure into intent and re-paste runtime context (the trigger's own id) that the engine envelope already provides at fire time. Use user-voice ("then delete this trigger"); the runtime knows the rest.
