---
name: Building a Trigger
description: Use when the user wants something to happen automatically — "every morning", "notify me when X happens", "watch for Y", recurring or event-driven background work. Covers cron vs on_event, the intent-vs-procedure rule, and notification discipline.
---

# Building a Trigger

How to guide a user from "I want this to happen automatically" to a working trigger. Cron format, frontmatter, and field reference live in the engine system prompt and the `create_trigger` tool description — don't restate them. The CLAUDE.md "trigger intent vs. procedure" rule is summarized below; see `docs/taxonomy.md` § Triggers for the worked example.

## When a trigger is the right answer

| User says | Right answer |
|---|---|
| "Every morning, send me…" | Trigger (cron) |
| "Notify me when my package ships" | Trigger (on_event), with a separate event-emitting source |
| "Check this once and tell me" | Just do it now, no trigger |

If the user only wants it to happen once, don't create a trigger.

## The most important rule: `run.intent` is intent, not procedure

A trigger's `run.intent` is **what the user would say** — one sentence in their voice. Everything about *how* (which API to hit, how to parse, what to retry, when to fall back) belongs in a knowhow file. The trigger thread looks up knowhow itself by calling `load_knowhow` at fire time — same as a chat session — so the trigger config has no per-trigger allow-list to configure. The HTTP API will accept a procedure-laden intent text and won't stop you — see `docs/taxonomy.md` § Triggers for the worked bad/good example.

### Don't paste procedure into `run.intent` to "make sure" the LLM sees it

The trigger thread inherits the same knowhow surface a chat thread has: the system prompt's intent registry advertises what's available, and the LLM calls `load_knowhow` when it judges a recipe relevant. Writing the procedure inline to bypass that lookup turns the intent into a recipe and the next person who reads the trigger config can't tell what the user originally asked for. Keep the intent in the user's voice ("send me a daily summary of open PRs") and let the LLM pull the procedure on demand.

## Cron vs. on_event vs. both

- **Cron** — "every morning at 8" / "weekdays at noon". Time-driven.
- **on_event** — "when X happens". Reactive. The event must already be emitted by something (an app, another trigger, an integration).
- **Both** — rare; usually means cron with a payload-shaped condition that should be on_event instead. Re-examine before doing this.

If the user says "notify me when X" and X isn't an event yet, you have two work items: (1) make X emit an event, (2) trigger on it. Tell the user that explicitly.

## `condition` — when to filter

Use `condition` when subscribing to a high-volume event but only caring about a slice. Example: subscribe to `EmailReceived` but only fire on emails from a specific sender. Without a condition, the trigger fires for every email and the LLM has to filter inside the run — wasteful and slow.

Don't use `condition` for logic that depends on external state (e.g. "only if this app's data file says X"). Conditions are pure payload filters. Stateful checks belong inside the run.

## Notification discipline

`send_notification` only fires when there's something the user actually wants to hear about. A morning summary that finds nothing new should produce no notification — silent success is the norm.

The scheduler auto-creates an error notification when a trigger fails. Don't double-notify on errors from inside the run.

## Where the thread lands: `go_to_review`

By default, trigger runs are unattended — their threads go straight to HISTORY when they finish, and only surface in REVIEW if the user follows up with a message. This is right for most cron triggers (silent imports, periodic syncs, idle nudges).

Set `go_to_review: true` when the trigger's *output is the point* — a daily summary the user is meant to read, an alert that needs acknowledgement, a scheduled report. The thread then surfaces in REVIEW on completion so it's not lost in HISTORY.

| User phrasing that answers it | Flag |
|---|---|
| "import my data", "sync X", "keep Y up to date" — silent housekeeping | omit (default false) |
| "put it in front of me", "make sure I see it", "I want to read this" | `go_to_review: true` |
| "summarize my week", "write a report I should look at" — output is the point | `go_to_review: true` |

A `send_notification` does **not** answer this question — notifications and review-surface are independent. A "notify me when X" trigger may or may not also need its thread to surface in review; the user has to tell you which.

If the user's request doesn't clearly land in one of the rows above, **ask** — see Question 5 below. The flag is snapshotted onto each run when it fires; toggling it later only affects future runs.

### Deep-link discipline (`app_id`)

Set `app_id` on a notification only when the notification is a direct call to action inside that specific app — tapping opens the app to act on the thing. Most cron triggers (reminders, nudges, summaries, status pings) should omit `app_id`. The owning-app `app_id` on the trigger itself is unrelated; it doesn't license the notification to deep-link.

- **Good:** a habit-tracker check-in trigger fires at 8:00 with "Check in for today" → set `app_id` to the habit-tracker id so tapping opens the check-in form.
- **Bad:** a 22:00 bedtime / wind-down nudge with body text only → leave `app_id` unset. It's a reminder, not a call to open an app.

## Edit, don't recreate

**Always look for an existing trigger first** (`list_triggers`) and modify it with `update_trigger`. Only call `create_trigger` when no comparable trigger exists. Recreating gives the new trigger a fresh `trigger_id`, which orphans the entire run history of the old one — the threads still exist in the database but no longer match the live trigger in the filter dropdown, in trigger-scoped reports, or anywhere else that joins by id. The user sees "no threads for current trigger" even though their workflow has been firing for months.

This applies to every shape of change:

| User says | What to do |
|---|---|
| "Change the cron to 9am" | `update_trigger(trigger_id, cron=...)` |
| "Rename it to X" | `update_trigger(trigger_id, name="X")` |
| "Switch it to fire on event Y instead" | `update_trigger(trigger_id, cron=null, on_event="Y")` |
| "Tweak the prompt" | `update_trigger(trigger_id, run={...})` |
| "Pause it" | `pause_trigger(trigger_id)` (or `update_trigger(..., paused=true)`) |
| "Make sure I see this one" / "Send to review" | `update_trigger(trigger_id, go_to_review=true)` |
| "Stop bringing this up — keep it in history" | `update_trigger(trigger_id, go_to_review=false)` |
| "Add another time it should run" | `update_trigger(trigger_id, cron=[existing..., new_expr])` — append to the cron array, don't make a sibling trigger |
| "Run it once more, like at 7pm tonight" | `update_trigger(trigger_id, cron=[existing..., one_shot_expr])`, then a follow-up `update_trigger` after it fires to remove the one-shot row. Don't create a duplicate trigger — even temporarily |

If you genuinely need a different trigger (different *workflow*, not a tweak of the same one), give it a clearly different name. Two live triggers named identically are a UX trap — the user can't tell them apart in any picker.

## Questions to settle with the user before creating

Don't call `create_trigger` from the user's first message. Most "create a trigger for X" requests leave at least one of these unsettled — confirm before writing the trigger. Skip questions only when the user has already answered them in the same turn.

1. **Recurring or one-shot?** "Notify me at 5pm today" is a one-shot — handle inline, don't create a trigger. Triggers are for things that should keep happening. If the user explicitly wants a one-shot trigger anyway (e.g. "create a test trigger that fires once in 2 min"), ask whether it should delete itself after firing — it won't on its own. See "One-shot triggers" below for the procedure.
2. **Cron or on_event?** "Every morning at 8" is cron. "When my package ships" is on_event. If the event doesn't exist yet, name the work (emit the event from somewhere, then trigger on it) and confirm.
3. **What's the run.intent in the user's voice?** One sentence the user would actually say. If you're tempted to write the procedure here, stop and put it in knowhow instead.
4. **Should it notify, and on what?** Default is silent — `send_notification` only fires when there's something the user wants to hear about. Confirm whether a successful run should notify, and what the message should look like.
5. **Surface to review or stay silent?** Always ask unless the user's phrasing clearly answers it (see the table in "Where the thread lands"). `go_to_review: true` for "I want to read this when it finishes" (daily summaries, scheduled reports, alerts that need acknowledgement); omit for silent housekeeping. A `send_notification` doesn't answer this — notifications and review-surface are independent.
6. **If updating an existing trigger:** confirm which one — see "Edit, don't recreate" above.

Don't ask all six in one wall — pick the ones the user's request actually leaves open. A request like "every Monday at 9am summarize my open PRs and put it in front of me" already answers cron, intent, and review-surface — only confirm the notification shape if it's not obvious. A request like "say hello once in 2 minutes" answers cron and intent but **not** review-surface — confirm before creating.

## One-shot triggers

Most one-shot requests ("remind me at 5pm today") should be handled inline without a trigger at all. Create a one-shot trigger only when the user explicitly asks for one (testing, demo, deliberate scheduling).

A one-shot trigger does **not** self-clean. After firing, the cron expression no longer matches anything, but the trigger row stays in the trigger list — visible in pickers, the filter dropdown, and `list_triggers` output — until something deletes it. There are two acceptable ways to handle this; pick one with the user before creating:

1. **Leave it.** Tell the user it will sit in the trigger list after firing and they can delete it from the UI when they want. Don't promise to clean it up.
2. **Ask the trigger to delete itself.** Add a sentence in the user's voice to the intent — e.g. `"Send me a hello notification, then delete this trigger."` Keep it user-voice; don't name `delete_trigger` or paste in the trigger id. The engine wraps each trigger fire in an envelope that already tells the running LLM its own id and that self-deletion is permitted, so the intent doesn't need to repeat any of that. Then confirm to the user that the trigger will delete itself after firing.

Don't claim "I'll delete it after it runs" without doing one of the above — see "Promising behavior the trigger doesn't have" below.

## Setup checklist

1. **Set timezone first** if not already set. Cron is 6 fields (`second minute hour day-of-month month day-of-week`) in the user's local timezone, DST-aware via IANA tz. The `create_trigger` tool refuses without a timezone.
2. **`list_triggers` first** to check whether an existing trigger should be updated instead of creating a new one.
3. **Decide cron vs. on_event** before writing the trigger.
4. **Write `run.intent` as the user would say it.**
5. **If the trigger needs a procedure-laden recipe, write it to a knowhow file.** Trigger-scoped recipes belong at `data/triggers/<slug>/knowhow/<descriptive>.md` (where `<slug>` is the trigger's kebab-case slug field — set it explicitly via `create_trigger`/`update_trigger`; if you don't, the engine derives one from the name on read but never persists it, so renaming the trigger silently moves the per-trigger knowhow path). Broadly reusable recipes go in shared `data/knowhow/` (see `building-knowhow.md`). The trigger thread discovers knowhow the same way chat does — via `load_knowhow` calls the LLM makes itself — so there is no `run.knowhow` field to populate. Any legacy `run.knowhow:[...]` you might see in old `TriggerCreated` payloads is silently dropped by the deserializer; rewrite the intent to either name the relevant knowhow inline ("see `system-knowhow/X`") or be rich enough to nudge discovery from the system-prompt knowhow listing. Make the file's `name` and `description` frontmatter precise so semantic discovery finds it.

## Common mistakes to avoid

- **Recreating instead of editing.** See "Edit, don't recreate" above. The single biggest source of orphaned thread history.
- **Recipe-in-text.** Putting procedure into `run.intent` instead of knowhow. See "The most important rule" above.
- **Cron when on_event fits.** Polling burns runs and adds latency. If an event exists, prefer it.
- **No knowhow file for a procedure the trigger clearly needs.** Without a discoverable knowhow file, the LLM re-derives the procedure every run and gets it slightly different each time. Write the recipe down — semantic discovery will surface it on the next fire.
- **Vague `name`/`description` frontmatter on a trigger-scoped knowhow.** Discovery is semantic, not by id, so a knowhow titled `notes.md` with `name: Notes` won't surface when the LLM is reasoning about an API call. Name the file by what it teaches (`openai-availability-check.md`), and write the `description` as the kind of question that should retrieve it.
- **Knowhow that recommends raw `curl`/`fetch` for an API the workspace already proxies.** When the recipe instructs the LLM to shell out with `curl -H "Authorization: Bearer $CRED_..."` (or the `requests`/`fetch` equivalent), the credential leaks into argv and tool transcripts. The right path is the `proxy_request` LLM tool against an entry in `data/config/apis.json` — see `system-knowhow/building-knowhow.md` § "Calling external APIs from a recipe".
- **Notifying on every tick.** A trigger that always notifies trains the user to ignore notifications.
- **Two live triggers with the same name.** Filter pickers and notification deep-links can't tell them apart. If you need two, name them differently.
- **Promising behavior the trigger doesn't have.** Only describe what's actually configured. Triggers do not self-clean — for a one-shot, follow the procedure in "One-shot triggers" above (either tell the user it will sit in the list, or ask the trigger to delete itself in the intent). Don't say "I'll delete it after it runs" unless you've actually wired that in.
- **Tool names or trigger ids in `run.intent`.** Intent is what the user would say, not how the LLM should act. Phrases like "call delete_trigger with trigger_id <uuid>" leak procedure into intent and re-paste runtime context (the trigger's own id) that the engine envelope already provides at fire time. Use user-voice ("then delete this trigger"); the runtime knows the rest.
