---
name: Triggers, Running an existing trigger once, off-schedule
description: Part of Triggers. Its index lists the sections it covers.
---

One part of `system-knowhow/triggers`, which lists every part.

## Running an existing trigger once, off-schedule

**`triggers(action="run", trigger_id)`.** That is the whole answer for a cron trigger. The CLI is `lucidos triggers run --id <uuid>`, the SDK is `lucidos.triggers.run(id)`, and the trigger's row in the panel has a **Run once** button. (Not to be confused with the Thread Queue panel's **Run now**, which force-admits an entry that is *already queued* and cannot create a fire.)

Link the trigger itself so the user can reach **Run once** on its row.

It is a real fire. It records `TriggerExecuted` / `TriggerCompleted` and updates the panel's `last_run` and OK/failed status. It runs under the trigger's own identity and program grants, and (for an `intent` run) its *trigger thread* and `go_to_review` routing. Downstream, nothing tells it apart from a scheduled fire. It returns once the run is admitted, not when the run finishes.

Three answers other than "started". Relay each as-is, never as a run:

- **Already running.** A fire of this trigger was already active or queued, so nothing new started: scheduled fires coalesce to at most one pending run per trigger. Tell the user that; do not claim you started one.
- **Paused.** Refused. Resuming runs nothing *on purpose*, so if the user wants both, do both. Resuming re-runs the missed-slot catch-up, so a cron slot from the past hour that never ran fires, and paused slots post no notice. That side effect is not a way to ask for a run, and you cannot predict it. A pause *you* just made counts at once: every trigger write is visible to the next call, so `triggers(action="pause")` then `run` in one turn is refused, not raced.
- **No cron schedule.** Refused: an event-only trigger has never fired without a payload. An intent run would find no `## Triggering Event` block, and a script run would get none of the `TRIGGER_EVENT_*` vars. Emit its event instead.

### Event-only trigger: emit the event

`events(action="emit", …)`, or `lucidos events emit <Type> --summary "…" --payload '{…}'` from a script. The emit goes through the same matcher, admission and run as a real event, so it is the faithful reproduction.

- **Per-entry `condition` filters still apply.** A payload that fails the condition matches nothing and you get silence, not an error. Read the `on` array from `triggers(action="list")` and build a payload that passes.
- **A condition on `actor` is not yours to pass.** The engine records you as the emitter, whatever the payload says (§ "Who emitted a domain event"). A trigger gated on `actor.kind` = `device` waits for the user. Ask them to press the button.
- Shape the payload like the real emitter's, not just enough to match: the run reads it (`## Triggering Event` for an intent, `TRIGGER_EVENT_PAYLOAD` for a script).
- The event is real and persisted, so every *other* subscriber fires too.
- **Event fires do NOT coalesce.** Each carries its own payload, so event fires keep strict FIFO, unlike the run action. Emit twice and the trigger runs twice, the second surfacing as an unexplained extra run minutes later. Check `threads(action="list")` (rows carry `trigger_id` and `status`) before you re-emit.

### Don't imitate the fire

Copying `run.intent` into `run_thread`, or running the trigger's script yourself with `run_python` / `run_bash`, looks like a run and isn't one. It records no `TriggerExecuted`, no `last_run` and nothing in the trigger's history. It gets none of the trigger-fire framing, its system rules or its program grants. Doing the work inline in the conversation is worse: there is no per-run thread to open, and a long or destructive procedure runs inside a chat turn.

Both are fine for **debugging** ("does the script still crash?"), if you call it that. A hand-run script gets none of `TRIGGER_EVENT_TYPE` / `TRIGGER_EVENT_PAYLOAD` / `TRIGGER_EVENT_ID` / `TRIGGER_EVENT_THREAD_ID`, so an event-driven one raises `KeyError` on the first lookup. Its `LUCIDOS_THREAD_ID` points at your conversation, so any `lucidos notify` lands in the wrong thread.

## What a trigger fire may not do to other triggers

**A trigger fire may act on itself and on nothing else, and even on itself it may not run itself.** The engine enforces it. Five scheduling tools are gated during a fire, and the `run` action is stricter still.

| Tool | On the firing trigger itself | On any other trigger |
|---|---|---|
| `triggers(action="create")` | refused | refused |
| `triggers(action="update")` | refused | refused |
| `triggers(action="delete")` | allowed | refused |
| `triggers(action="pause")` | allowed | refused |
| `triggers(action="resume")` | allowed | refused |
| `run` | refused | refused |

Self-delete and self-pause are allowed because they **terminate**: the fire acts on its own id and stops. Self-run is refused because it **recurses**. With the per-trigger concurrency cap at 1, a self-run cannot start a second copy. Each fire instead adds one entry to a queue that never drains.

The consequence for whoever writes a trigger: **a trigger whose purpose is to re-arm, reschedule, pause, or reconfigure ANOTHER trigger cannot work.** Nothing validates this when you create it. It arms clean, its panel row looks healthy, and it fires on time. The work fails only at the moment it runs.

### What to do instead

The common case: a recurring trigger is paused so a one-off run can take its place, and something has to turn it back on. In order of preference:

1. **Do not pause the recurring trigger at all.** Have it read a skip marker, a small file or a domain event. It then skips its own next run when the marker is set. The recurring trigger stays armed throughout, so nothing has to re-arm it, and the skip clears itself.
2. **If it must be paused, the re-arm is a human action.** A one-shot trigger that NOTIFIES the user the paused trigger needs resuming does work, because a notification is not a scheduling call. Tell the user plainly that this is a reminder, not an automation.
3. **Do not route around the guard with `curl`.** The `run` refusal holds at the HTTP layer too, and the engine states the other guards explicitly. Posting to the engine API to reach another trigger is out of bounds.

## Questions to settle with the user before creating

Don't call `triggers(action="create")` from the user's first message. Most "create a trigger for X" requests leave at least one of these unsettled, so confirm before you write the trigger. Skip a question only when the user already answered it in the same turn.

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

A one-shot trigger does **not** self-clean. After it fires, its cron matches nothing, but the row stays in the trigger list until something deletes it. It shows in pickers, the filter dropdown and `triggers(action="list")` output. Pick one of two ways with the user before you create it:

1. **Leave it.** Tell the user it will sit in the trigger list after firing, and they can delete it from the UI. Don't promise to clean it up.
2. **Ask the trigger to delete itself.** Add a sentence in the user's voice to the intent, for example `"Send me a hello notification, then delete this trigger."` Don't name `triggers(action="delete")` or paste in the trigger id. The engine's fire envelope already tells the running LLM its own id and that self-deletion is permitted. Then tell the user the trigger will delete itself after firing.

Never claim "I'll delete it after it runs" without one of the above.

## On-disk trigger definition (`trigger.toml`)

**The scheduler never reads this file.** `data/triggers/<slug>/trigger.toml` is a
**derived read-model** of the trigger's definition. It mirrors the durable
subset of its config (`name`, `slug`, `schedule`, `timezone`, `run`, `on`,
`app_id`, `go_to_review`, `group_id`, `side_effect_grant`, `coding_agent_grant`,
`context_ceiling`, `mcp_servers`, `model`, `reasoning_effort`, `provider`). A
trigger on the account trigger model omits the last three. One on the workspace
ceiling omits `context_ceiling`, and an ungranted one omits
`coding_agent_grant`.

The engine maintains it from the trigger events: written on
create/update, removed on delete, rebuilt from events on boot (ADR 0019).
Runtime and identity fields (`id`, `last_run`, `last_run_status`, `paused`) are
left out. It is **not version-controlled**: the engine adds
`data/triggers/*/trigger.toml` to the workspace repo's local `.git/info/exclude`.

Events are authoritative: the scheduler runs off the event-replayed config, and
this file mirrors that config, never feeds it. Two rules follow:

- **Never hand-edit it.** The edit writes a file that reads correctly and changes
  nothing the scheduler sees; the next trigger event or restart overwrites it.
  Change triggers via `triggers(action="create")`/`triggers(action="update")` (or the UI), which emit
  the events the projection follows.
- **Never verify from it.** After a config change, re-read the trigger from
  `triggers(action="list")`, the live registration. Reading `trigger.toml` back off disk
  proves only that it was written, so a change the scheduler never saw still
  verifies green.

Each fire is recorded as events (`TriggerExecuted` + `TriggerCompleted`, plus any
*domain event* the run emits). The trigger's row in the triggers panel shows the
**last run's OK/failed status** next to its timestamp. There is no built-in
run-history view. For detail on a threadless trigger's runs (what it found, when,
why a run failed), ask the *Lucidos Agent*, which reads the events via
`events(action="query")`. Or build an *app* on the trigger's events (`lucidos.events`).

The file exists so a trigger is inspectable: the Plugins panel's installed-plugin
file links point at it for plugin-shipped triggers. It also lets a *plugin* SHIP a
trigger by declaring one, see `plugins.md`.

### Renamed trigger → stale `run.path`

The folder is named by the trigger's `slug`, never by its current `name`, so
renaming moves nothing. The slug changes through an explicit `slug` via the CLI / HTTP
API, or a delete-and-recreate. That relocates only `trigger.toml` to
`data/triggers/<new-slug>/` and deletes the old copy. `scripts/`, `knowhow/`
and the registered `run.path` all stay under the old slug. The tell: a
`trigger.toml`-only folder beside a `scripts/`-only one.

Repair, in order:

1. `git mv` the old slug's `scripts/` and `knowhow/` (whichever exist) into
   `data/triggers/<new-slug>/`.
2. `triggers(action="update", trigger_id, run={type:"script", path:"triggers/<new-slug>/scripts/run.py"})`.
   Only the event re-points the scheduler.
3. Confirm the new path in `triggers(action="list")`, then delete the old folder. Deleting
   before step 2 lands removes the script the scheduler is still calling.

A broken run reports `Script not found: data/scripts/<path>`. That is the last
candidate in the path-resolution fallback, not the configured path, which
`triggers(action="list")` shows.

## Setup checklist

1. **Set timezone first** if not already set. Cron is 6 fields (`second minute hour day-of-month month day-of-week`) in the user's local timezone, DST-aware via IANA tz. The `triggers(action="create")` tool refuses without a timezone. For anything beyond a plain daily or weekly time, read § "Writing cron expressions" above: the AND/OR split, the nth-weekday and last-weekday recipes, and the combinations the engine rejects.
2. **`triggers(action="list")` first** to check whether an existing trigger should be updated instead of creating a new one.
3. **Decide cron vs. `on` (and whether `on` needs multiple entries)** before writing the trigger.
4. **Write the knowhow file, THEN `run.intent` as the user would say it.** The ordering is the rule (§ "Write the knowhow file FIRST, then the intent"). Reusable recipes go in shared `data/knowhow/` (see `building-knowhow.md`); trigger-scoped ones go at `data/triggers/<slug>/knowhow/<descriptive>.md`. `<slug>` is minted from the name at creation (unless given), stored on `TriggerCreated`, and unchanged by a rename, so the folder keeps the old name. The LLM tools take no `slug`; the CLI (`lucidos triggers create --slug`) and HTTP API do. Changing it strands `knowhow/` and `scripts/` under the old slug (§ "Renamed trigger → stale `run.path`").

   There is no `run.knowhow` field: the trigger thread finds knowhow via its own `load_knowhow` calls, as chat does. The deserializer silently drops a legacy `run.knowhow:[...]` in old `TriggerCreated` payloads. Rewrite such an intent to name the knowhow inline ("see `system-knowhow/X`"). Or make it rich enough to nudge discovery from the system-prompt knowhow listing. Give the file precise `name` and `description` frontmatter so semantic discovery finds it.

   Shared `data/knowhow/` is what you can write first. Trigger-scoped is the
   exception to the ordering: write that one *after* `triggers(action="create")` returns,
   because only then is `<slug>` authoritative.
5. **List the MCP servers the intent calls** in `mcp_servers`, and only those. An unlisted server's calls are refused in the fire (§ "MCP servers the fires use: `mcp_servers`").

## Common mistakes to avoid

- **Recreating instead of editing.** The biggest source of orphaned thread history. See "Edit, don't recreate" above.
- **Hand-editing `trigger.toml`.** The scheduler never reads it, so the edit no-ops and the next trigger event or restart overwrites it. Change the config with `triggers(action="update")`, then verify against `triggers(action="list")`. See "On-disk trigger definition" above.
- **Resuming a paused trigger to "run it now", or hand-rolling the run.** Resume runs nothing by itself. Use `triggers(action="run")`, or emit the subscribed event for an event-only trigger. See "Running an existing trigger once, off-schedule" above.
- **A trigger that manages another trigger.** Refused at fire time, and nothing catches it earlier. See § "What a trigger fire may not do to other triggers".
- **Patching around a `[REMOVED KNOW-HOW]` block.** Rewriting the missing recipe from memory hides the removal and drifts each run. Find the current knowhow, tell the user, and suggest they edit the trigger. See § "Know-how a plugin removed".
- **Recipe-in-text.** Procedure in `run.intent` instead of knowhow, almost always because the knowhow file was never written first. See "Write the knowhow file FIRST, then the intent" above.
- **A webhook condition without the `payload.` prefix.** `{"action": "completed"}` matches nothing, and nothing warns you. See § "The envelope: a webhook's body lands under `payload`".
- **An MCP server the intent uses but the trigger does not list.** Every call to it is refused in the fire, with a notification. See § "MCP servers the fires use: `mcp_servers`".
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
