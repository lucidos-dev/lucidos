---
name: Triggers, Actor
description: Part of Triggers. Its index lists the sections it covers.
---

<!-- domain-event: OrderPlaced: Example domain event emitted by an order-entry app. -->
<!-- domain-event: OrderQueuedForShipping: Example domain event emitted by an order-processing trigger. -->
<!-- domain-event: EmailReceived: Example domain event emitted by an email-ingestion integration. -->

One part of `system-knowhow/triggers`, which lists every part.

## Actor

Set `actor` on an intent trigger to say whose action a fire carries: the person who wrote the comment, filed the ticket or sent the message. The value is one **field path** to an email in the fire's event payload. It uses `condition` syntax, so a webhook's fields sit under `payload.` (§ "The envelope").

| `actor` | Payload it reads |
|---|---|
| `"payload.sender.email"` | A webhook whose body names the sender's email |
| `"author_email"` | A domain event your translation script emits |

How a fire uses it:

- **The fire records the email, never a user.** It goes on the fire's origin: on `TriggerStarted`, and on the answer when a keyed fire answers a question. The thread credits the *user* who holds that email, and moving an email to another user moves the credit.
- **An email no user holds credits nobody.** The thread still runs. It just names the trigger alone.
- **No email, no credit.** A path that resolves to nothing, or to a value that is not one email, records none. The email is trimmed and lowercased.
- **It is attribution, never authority.** The fire runs with the workspace's credentials either way. Use the trigger's `condition` to decide whose actions may fire it.

Some sources send user ids rather than emails. Give them a script trigger that looks the id up and emits a domain event with the email. Point the intent trigger's `actor` at that field.

Only intent triggers take an actor: a script fire starts no thread to credit. `actor: null` or `""` clears it.

This `actor` is not the engine-written `actor` on a domain event (§ "Who emitted a domain event: `actor`"). That one records which caller emitted the event. This one names a person inside the event's own payload.

## Notification discipline

Call `send_notification` only when there is something the user wants to hear about. A morning summary that finds nothing new sends no notification: silent success is the norm.

End each trigger run with `request_read`:

- **`read: false`** for routine reports, even when the trigger asks for one each run. Use false after sending a notification, unless the reply holds a failure the notification omitted.
- **`read: true`** only for something new the user would miss, such as a finding, a failure or a decision only they can make. The engine reviews a yes and sees what the run already sent. If it agrees, the thread waits under Review until the user reads the reply.

The same judge decides when the run leaves the read decision open. Use `read: false` to settle routine runs without a judge call.

A trigger that orchestrates *sub-threads* or waits on events can wake many times. The same judge reviews a yes on these wakes until a user or agent message starts a new turn. Use `read: false` for progress. Say `read: true` only for something the user should read before the run finishes, such as a failure.

The scheduler auto-creates an error notification when a trigger fails, so don't double-notify on errors from inside the run. Boot also sends one notification per interrupted run, opening its cut thread and explaining whether a retry was queued. Let that report carry the interruption; the retry sends notifications only for its own findings or failures.

## Where the thread lands: `go_to_review`

By default, trigger runs are unattended: their threads go straight to Archive when they finish. They surface in the Current section only if the user follows up with a message. That suits most cron triggers (silent imports, periodic syncs, idle nudges).

An interrupted run is an exception: boot restores its cut thread to the inbox, preserving its pin and failed status. This applies even when **Send directly to Archive** is on. Its retry runs in a new thread with the normal routing below.

A run that stops to ask the user a question, or to ask permission, always surfaces in Current, whatever `go_to_review` says. It stays there until the user answers or stops it.

A run with a read request also lands in Current and is listed under Review until the user reads its reply. The engine reviews the run’s yes before recording the read request.

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

An intent trigger fires on the account's *trigger model* (the `trigger_model`
preference, Settings → Models → Chat & triggers) unless it says otherwise. That
default is separate from the chat model, so Opus in chat does not make every
unattended fire run on Opus. Set `model` to pin it to a specific
chat model and `reasoning_effort` to pin its thinking budget
(`none|low|medium|high|xhigh|max`). The two are independent: pinning the model
leaves the effort on the account setting, and the reverse.

To unpin, send null: `triggers(action="update", trigger_id, model=null, reasoning_effort=null)`.
The trigger then follows the account trigger model again. The CLI spells it
`--model null --reasoning-effort null`, and `provider` clears the same way. An
update that omits a field leaves that pin as it is.

Script triggers have no model. They run no LLM, so these fields and `provider`
below are ignored there, and the form hides them.

| User phrasing | What to set |
|---|---|
| "use something cheap for this", "it's just a digest" | a low-cost model, often with a low `reasoning_effort` |
| "this one needs to be thorough", "use the best model" | the stronger model, and usually a higher `reasoning_effort` |
| nothing about models | omit both, so the trigger follows the account trigger model |

Pin a model only when the user asked for one. A pinned trigger stops following
the account trigger model, so a workspace-wide model change no longer reaches it. A
model pin with no `reasoning_effort` runs that model at the account's tier for
it, else its default effort.

Like the `chat_model` preference, the model id is **not checked against the
registry at save**, since a model can be deleted later. A wrong id
fails at fire time, as a normal trigger-failure notification. Use
`manage_models(action='list')` to see the real ids.

The model and effort a run actually used are recorded on its `TriggerStarted`
event. So the trigger's thread shows what it ran on. A follow-up there
continues on the same model rather than snapping to the account chat model.

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

Write the **`message` as content only, and never restate the `title` in it.** The in-app toast, the inbox detail and the OS push each show the title in their own right. So a body that opens by repeating the title shows it twice. Use a bare sentence for a single item and `"• "`-prefixed lines for a list. The toast shows each line as written, under the bold title. See `system-knowhow/notifications/3-the-presencecheck-protocol.md` §4.

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

See `system-knowhow/js-sdk/lucidos-preferences-user-settings.md` § `lucidos.notifications` for the full `NavigateUi` target list (panels, apps, threads, files, triggers, creation forms, URLs).

#### Where the LLM finds `event_id`

When a trigger fires from a `BusEvent::Thread` match, the engine appends a `## Triggering Event` block to the trigger's user message. Above the JSON payload, a line like:

```
Source event id: 7a9c2c5f-…
```

…carries the UUID of the event that fired the trigger. Pass it as `send_notification`'s `event_id`. The push tap then deep-links to that exact event, and the question card pulses on land.

**Only a thread event qualifies.** A workspace domain event (anything `events(action="emit")` wrote, such as `E2ETestsPassed`) gets a `Source event id` line too, but it lives in no thread. No transcript can show it, so `send_notification` refuses it as `event_id`, and `lucidos notify` refuses it as `--event-id`. Omit `event_id` for those, and the tap opens the notification card. The same refusal covers an event from a different thread than the one the tap opens.

For schedule (cron) triggers there is no source event, so no `event_id`. For on-event triggers that notify about *a different* event (e.g. fire on `CodingAgentIdled` but notify about the last `UserQuestionAsked`), look the right event up yourself with `events(action="query")` and use that id.

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

A trigger that a plugin ships is the exception. Its folder belongs to the plugin, so its state goes under `data/artifacts/<plugin-id>/` instead. See `system-knowhow/plugins/authoring-a-marketplace.md` § "Where a plugin keeps its runtime state".

### Script trigger env vars

When the engine fires a script trigger that subscribes to a domain event, it sets these env vars before it runs the script. A schedule fire sets none of them, since there is no source event.

| Env var | Set when | What it holds |
|---|---|---|
| `TRIGGER_EVENT_TYPE` | Always on event fires | The matched event name (e.g. `UserQuestionAsked`). Use as a fallback title or when the script genuinely needs to branch on type. |
| `TRIGGER_EVENT_PAYLOAD` | Always on event fires | The source event's payload, serialized as JSON. Parse with `json.loads(os.environ["TRIGGER_EVENT_PAYLOAD"])`. |
| `TRIGGER_EVENT_ID` | When the source event has a row id | The `events.id` (UUID) of the source row. Pass it to `lucidos notify --event-id`, beside `--thread-id "$TRIGGER_EVENT_THREAD_ID"`, so the push tap scroll-and-pulses the exact card. A domain event has no thread, so leave it out then: the engine refuses an event outside the linked thread. |
| `TRIGGER_EVENT_THREAD_ID` | Only for *thread-scoped* source events | The thread the source event lives on. Pass to `lucidos notify --tap navigate --thread-id` so the push deep-links to the originating conversation instead of the trigger's own thread (which is `LUCIDOS_THREAD_ID`). |

The trigger's own thread is `LUCIDOS_THREAD_ID`, the same env var every spawned subprocess gets. `TRIGGER_EVENT_THREAD_ID` is the *source* event's thread, a different one. Mix them up and the push deep-links into the trigger's own thread instead of where the user needs to act.

**A script trigger gets a credential only when its script file names it.** Write `os.environ["CRED_GITHUB"]` literally, or declare a name built at run time with a header comment, `# lucidos-credentials: CRED_A CRED_B`. Run `lucidos credentials list` for the names. See `system-knowhow/best-practices/workspace-layout.md` § Credentials in subprocesses.

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

