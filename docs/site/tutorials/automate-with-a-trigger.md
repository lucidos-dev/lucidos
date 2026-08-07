# Automate with a trigger

A **trigger** makes something happen on its own — on a schedule ("every weekday at
8") or in response to an event ("when my package ships"). Like apps, you create one
by describing it in chat; the Lucidos Agent confirms a few details and wires it up.
This tutorial covers when a trigger is the right tool, the one rule people always get
wrong, and how to keep it from becoming notification noise.

!!! info "Prerequisite"
    A running Lucidos workspace with an LLM provider configured — see the
    [Quickstart](../quickstart.md).

## 1. Is a trigger even the right answer?

| You want… | Right answer |
|---|---|
| "Every morning, send me…" | Trigger (schedule) |
| "Notify me when my package ships" | Trigger (event) — something has to emit that event |
| "When either X or Y happens, do Z" | **One** trigger with two trigger subscriptions, not two triggers |
| "Check this once and tell me" | Just do it now — no trigger |

If you only want it to happen once, don't create a trigger. A one-off ("remind me at
5pm today") is handled inline.

## 2. Schedule vs. event

- **Schedule (cron)** — time-driven. "Every morning at 8", "weekdays at noon". Cron in
  Lucidos is six fields (`second minute hour day-of-month month day-of-week`) in your
  local timezone, DST-aware — so **set your timezone first** if you haven't; trigger
  creation refuses without one.
- **Event (`on`)** — reactive. "When X happens." The event must already be emitted by
  something — an app, another trigger, an integration. The `on` list can hold several
  subscriptions, each with its own optional payload filter, so *one* workflow can react
  to several event types without duplicating the trigger.

If you say "notify me when X" and nothing emits X yet, that's two pieces of work: make
X emit an event, then trigger on it. The agent will tell you so rather than pretend.

## 3. The rule everyone gets wrong: intent is not procedure

A trigger's **intent** (`run.intent`) is *what you would say* — one sentence in your
voice. Everything about *how* — which API to call, how to parse it, what to retry,
when to fall back — belongs in a **knowhow** file, which the trigger discovers at fire
time the same way a chat does. (See [Concepts](../concepts.md#intent-knowhow-and-scripts).)

It's tempting to paste the whole recipe into the intent "so the model definitely sees
it." Don't — it turns the intent into a script and the next person can't tell what you
originally asked for.

> **Procedure in the intent (wrong):** "Check whether gpt-5.5 is available. GET
> `https://api.openai.com/v1/models` with `Authorization: Bearer $OPENAI_API_KEY`,
> scan `data[].id` for ids starting with `gpt-5.5`, on 401 fall back to a web search…"
>
> **Intent in your voice (right):** "Notify me when an OpenAI model with id prefix
> `gpt-5.5` becomes available via the API. Once notified, disable this trigger."
>
> …with the GET/parse/fallback recipe written once into a knowhow file the trigger
> finds by description.

When the endpoint changes you fix one knowhow file, not every trigger that touches it.

!!! tip "Mechanical work? Use a script trigger."
    When the job is a fixed transformation with no judgement call — "on any subscribed
    event, notify with the payload's title and message" — the trigger's `run` can be a
    **script** run directly, with no LLM call per fire. Keep it an **intent** when the
    wording should adapt to context or the workflow branches on prior results.

## 4. Notification discipline

The fastest way to train yourself to ignore Lucidos is a trigger that notifies on
every tick. The norm is **silent success**: `send_notification` should fire only when
there's genuinely something you want to hear about. A morning summary that finds
nothing new sends nothing. And you don't need to handle failures — the scheduler
auto-creates an error notification when a trigger run fails, so don't double-notify.

## 5. Where the run lands: review vs. archive

By default, trigger runs are unattended — their threads go straight to **Archive** and
only resurface if you follow up. That's right for silent housekeeping (imports, syncs,
nudges).

Set **`go_to_review: true`** when the *output is the point* — a daily summary you're
meant to read, an alert that needs acknowledgement, a scheduled report. The run then
surfaces in the Current section when it finishes, so it isn't lost.

A notification doesn't answer this question — notifications and review-surface are
independent. If your phrasing doesn't make it obvious, the agent will ask.

## 6. Create it by describing it

Put it together and just say it:

> "Every weekday at 9am, summarize my open PRs and put it in front of me."

That one sentence already answers schedule (cron), intent (the summary), and
review-surface ("put it in front of me" → `go_to_review: true`). The agent confirms
anything still open — usually whether and how it should notify — writes any needed
knowhow file *first*, then creates the trigger.

!!! warning "Edit existing triggers — don't recreate them"
    To change a trigger ("make it 8am instead", "also fire when Y happens"), the agent
    *updates* the existing trigger. Recreating it mints a new id and orphans the old
    one's entire run history — months of runs that no longer match the live trigger in
    any picker. Append to the `cron` or `on` list; don't make a sibling.

!!! note "Unattended side-effects need your sign-off"
    If you have **Command Safety** turned on (Settings → Permissions; off by default),
    a trigger can't pause to ask you about an irreversible action — sending email, a
    mutating API call, a cloud-CLI change. You pre-authorize those per trigger via its
    **Allowed side-effects** grant. The agent can't set the grant for you (so it can't
    widen its own authority) — it'll tell you which box to tick.

## Going deeper

- **Triggers** (`system-knowhow/triggers.md`): cron vs. event in
  depth, multiple subscriptions per trigger, conditions, script triggers, one-shots, and
  notification routing.
- **Concepts → Intent, knowhow, and scripts** ([here](../concepts.md#intent-knowhow-and-scripts)) — the
  intent/knowhow split this tutorial leans on.
