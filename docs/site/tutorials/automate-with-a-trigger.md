# Automate with a trigger

A **trigger** runs work on its own, on a schedule ("every weekday at 8") or in
response to an event ("when my package ships"). You create one by describing it
in chat. The Lucidos Agent confirms a few details and sets it up. This tutorial
covers when to use a trigger, how to write its intent, and how to keep its
notifications useful.

!!! info "Prerequisite"
    A running Lucidos workspace with an LLM provider configured. See the
    [Quickstart](../quickstart.md).

## 1. Is a trigger the right answer?

| You want… | Right answer |
|---|---|
| "Every morning, send me…" | Trigger (schedule) |
| "Notify me when my package ships" | Trigger (event); something has to emit that event |
| "When either X or Y happens, do Z" | **One** trigger with two trigger subscriptions, not two triggers |
| "Check this once and tell me" | Ask now, without a trigger |

If you only want it to happen once, don't create a trigger. Lucidos handles a
one-off ("remind me at 5pm today") inline.

## 2. Schedule vs. event

- **Schedule (cron)**: time-driven, such as "every morning at 8" or "weekdays at
  noon". Cron in Lucidos has six fields
  (`second minute hour day-of-month month day-of-week`) in your local timezone,
  DST-aware. **Set your timezone first**: trigger creation refuses without one.
- **Event (`on`)**: reactive, "when X happens". Something must already emit the
  event: an app, another trigger, or an integration. The `on` list can hold
  several subscriptions, each with its own optional payload filter. So *one*
  trigger can react to several event types.

If nothing emits X yet, "notify me when X" is two pieces of work: make X emit an
event, then trigger on it. The agent tells you when this is the case.

## 3. Intent is not procedure

A trigger's **intent** (`run.intent`) is *what you would say*: one sentence in
your voice. Everything about *how* belongs in a **knowhow** file: which API to
call, how to parse it, what to retry, when to fall back. The trigger finds that
knowhow at fire time, the same way a chat does. (See [Concepts](../concepts.md#intent-knowhow-and-scripts).)

Keep the procedure out of the intent. A procedure turns the intent into a
script, and the next reader can no longer see what you originally asked for.

> **Procedure in the intent (wrong):** "Check whether gpt-5.5 is available. GET
> `https://api.openai.com/v1/models` with `Authorization: Bearer $OPENAI_API_KEY`,
> scan `data[].id` for ids starting with `gpt-5.5`, on 401 fall back to a web search…"
>
> **Intent in your voice (right):** "Notify me when an OpenAI model with id prefix
> `gpt-5.5` becomes available via the API. Once notified, disable this trigger."
>
> …with the GET, parse, and fallback steps written once into a knowhow file that
> the trigger finds by description.

When the endpoint changes, you update one knowhow file, and every trigger that
uses it picks up the change.

!!! tip "Script triggers for mechanical work"
    For a fixed transformation with no judgement call, the trigger's `run` can be
    a **script**. An example is "on any subscribed event, notify with the
    payload's title and message". Lucidos runs the script directly, with no LLM
    call per fire. Use an **intent** when the wording should adapt to context or
    the workflow branches on prior results.

## 4. Notification discipline

The norm is **silent success**: call `send_notification` only when there is
something you want to hear about. A morning summary that finds nothing new sends
nothing. The scheduler creates an error notification when a trigger run fails,
so don't add your own.

## 5. Where the run lands: review vs. archive

By default, a trigger run is unattended. Its thread goes straight to **Archive**
and resurfaces only if you follow up. That suits silent housekeeping (imports,
syncs, nudges).

Set **`go_to_review: true`** when you need to see the output: a daily summary to
read, an alert to acknowledge, a scheduled report. The run then appears in the
Current section when it finishes.

Notifications and `go_to_review` are independent. If your phrasing leaves it
unclear, the agent asks.

## 6. Create it by describing it

Describe the whole trigger in one sentence:

> "Every weekday at 9am, summarize my open PRs and put it in front of me."

That sentence sets the schedule (cron), the intent (the summary), and the review
surface ("put it in front of me" → `go_to_review: true`). The agent confirms
anything still open, usually whether and how it should notify. It writes any
needed knowhow file *first*, then creates the trigger.

!!! warning "Edit, don't recreate"
    To change a trigger ("make it 8am instead", "also fire when Y happens"), the
    agent *updates* the existing trigger. Recreating it mints a new id and orphans
    the old one's run history. Those runs no longer match the live trigger in any
    picker. Append to the `cron` or `on` list instead of making a sibling.

!!! note "Unattended side-effects need your sign-off"
    With **Command Safety** on, a trigger can't pause to ask you about an
    irreversible action. Command Safety is off by default (Settings →
    Permissions). Irreversible actions include sending email, a mutating API
    call, or a cloud-CLI change. You pre-authorize those per trigger with its
    **Allowed side-effects** grant. Only you can set the grant, and the agent
    tells you which box to tick.

## Going deeper

- **Triggers** (`system-knowhow/triggers.md`): cron vs. event in
  depth, multiple subscriptions per trigger, conditions, script triggers, one-shots, and
  notification routing.
- **Concepts → Intent, knowhow, and scripts** ([here](../concepts.md#intent-knowhow-and-scripts)):
  the intent and knowhow split this tutorial uses.
