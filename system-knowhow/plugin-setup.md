---
name: Plugin Setup
description: Use when a thread asks to set up a newly installed plugin, or to set up one again after an update. Covers reusing what an earlier run did, finding the author's instructions, planning the steps, doing the wiring, and asking for what only they can provide.
---

# Plugin Setup

This thread exists to finish setting up a plugin that was **just installed or updated** in this workspace. The opening request names the plugin, and says which of the two happened. "Set up the newly installed Super Slides plugin" is a first install; "Set up Super Slides again" is an update whose setup instructions changed. The plugin author left setup instructions, and your job is to carry them out with the user, who is reading this thread right now.

## 1. On an update, start from what the last run already did

**Skip this whole section on a first install.** The opening request says "newly installed", there is nothing to reuse, and § 2 is where you start.

On an update the user has been here before. Asking them everything again is the failure this section exists to prevent, so work out what is genuinely new before you ask anything:

- **Diff the author's two setup texts.** Query `PluginInstalled` for this plugin and take the newest two. The `setup` value of each sits at `payload.data.manifest.manifest.setup` (§ 2 walks that path). What the author changed between them is the work; the rest is already done.
- **Read the record of the last run.** Query `PluginSetupCompleted` and take the newest whose `plugin` matches this one. It carries the choices the user made and what was deliberately skipped, which a fresh reading of the author's text cannot recover.
- **Fall back to the earlier thread only when there is no record.** The previous `PluginInstalled` event carries its setup thread's id at `payload.data.manifest.setup_thread_id`, one level above the raw manifest, and `query_events` reads that thread by `thread_id`. It is a whole transcript, so reach for it last, and only for the questions the diff says still matter.

### Verify before you skip

**A record is a hint, never authority.** Skip a step only once you have checked that the thing it would create is already there:

| Step would create | Check it with |
|---|---|
| a trigger | `lucidos triggers list` |
| a webhook | `lucidos webhooks list` |
| a credential | `run_bash`, looking for a `CRED_<NAME>` entry in `env`, never printing it |
| a config entry | read the file, e.g. `data/config/apis.json` |

This is what makes the fast path safe. The plugin may have been uninstalled and reinstalled, or the user may have deleted a trigger. A plugin set up before records existed has none at all. In each case the record says "done" and the workspace says otherwise, and the workspace is right.

Then tell the user in one line what you found still in place, and ask only about what changed. Two cards, not the whole interview.

## 2. Reference the author's setup instructions

The author's instructions are **not** in this thread's opening message — they live in the durable `PluginInstalled` event for the plugin. Retrieve them:

- Call the `events` tool with `action=query`, `event_type=PluginInstalled` (newest first). The version you are setting up is the most recent one.
- In each returned event, the plugin's manifest is nested inside `payload`. The author's `setup` text and the plugin `name`/`version` live at **`payload.data.manifest.manifest`**. The `events` tool wraps the stored event as `{ type, data }`, and the raw manifest sits one level below the event's own `manifest` map. The plugin id is easier: **`payload.data.id`**. Match that manifest's `name` to the plugin named in this thread's request, then read its `setup` value. That text is the author's setup steps, written as instructions about what to do with the user.

You can re-query this event on any later turn if you need the instructions again — it is immutable, so it never goes stale.

**A `condition` names a shorter path than the one above.** The paths here are for *reading* a stored row, envelope and all. A trigger's `on` entry, or an `await_event`, is matched against that payload with the envelope already unwrapped. So the same two values are `id` and `manifest.manifest.version`, with no `payload.data.` in front. Write a read path into a condition and it resolves to nothing, silently. The engine warns at subscription time and names the real path, but knowing the rule beats reading the warning.

## 3. Plan the steps as a todo list

Turn the author's instructions into a `todo_write` list (one item per concrete step) before you start. This shows the user your plan, lets them watch progress tick along, and keeps the plan visible across turns. Flip each item to `in_progress` as you start it and `completed` as you finish — at most one `in_progress` at a time.

## 4. Do the wiring you can; ask for what you can't

- **Do it yourself** wherever the step is something you can carry out (writing a config file, pasting an `apis.json` snippet for a signer plugin, creating a trigger, etc.).
- **Ask the user directly** for anything only they can provide — credentials, account choices, confirmations. Prefer `ask_user_question` for choice-shaped questions; use `request_credential` (never chat) for secrets.

### Webhooks: the plugin cannot ship one, so you create it here

A plugin ships files. A webhook is a row in the `webhooks` table, so it never travels in the bundle. When the plugin ships a trigger that subscribes to an event a third party sends, creating the hook is your job.

That trigger is already live. Install auto-registers it, subscribed to an event type nothing emits until you finish. Nothing warns anyone if you stop halfway, so treat the hook as the step that makes the plugin work at all.

Work in this order:

1. **Get the shared secret.** Use `request_credential`, never chat. The sender generates it or the user invents one. Either way the same value must sit on both sides.
2. **Create the hook.** Run `lucidos webhooks create --name "<plugin> <sender>" --event-type <TheEventTheTriggerSubscribesTo> --hmac '{...}'`, with `credential` naming the credential you just saved. `system-knowhow/lucidos-cli` § Webhooks carries the exact `--hmac` shape per sender, plus the header allow-list. Take the event type from the plugin's `trigger.toml`, not from your reading of the author's prose. A subscription matches the string exactly, and a near miss fires nothing.
3. **Read the shipped trigger's condition against the delivery shape.** A delivery is always `{summary, headers, payload}`, with the sender's own body under `payload`. So a condition reads `payload.action`, never a bare `action`, and a header reads `headers.X-GitHub-Event`. An author who wrote theirs from the sender's API docs got this wrong, and it fails silently: the hook delivers, the trigger never fires. Fix the `trigger.toml` before you report done.
4. **Read back the URL.** `lucidos webhooks list` prints the delivery path. The full address is `{host}:{hook_port}/<workspace-slug>/<webhook-id>`, and the id is minted at create time, so the value the sender needs does not exist before step 2.
5. **Check the hook socket is reachable.** Deliveries land on the gateway hook socket, not the main surface. On a machine with no remote access set up, nothing outside can reach it and every delivery fails at the sender. See `system-knowhow/remote-access` for exposing that one port.
6. **Hand the user the sender-side steps.** Only they can paste the URL into GitHub, Stripe or whatever sends the event, under an account you cannot reach. Give the URL in a `<copy>` tag, with the content type, the name of the secret field, and which events to subscribe to.
7. **Verify before you report done.** `lucidos webhooks list` must show the hook, and the plugin's trigger must name the same event type. State both. An unverified hook leaves a silent trigger behind.

An unsigned hook prints a bearer token once, at create time, because only the digest is stored. Pass it to the user the same way, and say plainly that it cannot be read back.

## 5. Communicate in-thread, no notifications

The user is watching this thread. Communicate entirely through your normal replies here. Do **NOT** call `send_notification` — a toast or push for setup steps they are already reading is noise.

## 6. Confirm when done

When every step is complete, give a short confirmation of what is now ready to use. When the plugin ships an app, give it as a **clickable app link** so the user can open it in one click straight from this thread — a markdown link using the `app:<id>` scheme, e.g. `[Super Slides](app:super-slides)` (the `<id>` is the app's folder name under `data/apps/`). A bare prose mention of the app name is not a link. Also mention the trigger that will run or whatever else the setup enabled.

**Finishing this thread is what flips the plugin's card.** Its button reads **Setup** while the thread is running or waiting for the user's answer, and **Open** once it is neither. So a thread parked on a question nobody answers keeps the card on Setup for good. Ask what you genuinely need, then finish.

## 7. Record what you set up

The next update spawns another setup thread, and § 1 is what stops it re-asking everything. That section only works if this one runs, so emit the record before you finish:

```
emit_event("PluginSetupCompleted", {
  "summary": "Set up Super Slides 0.5.2: 1 trigger, 1 credential",
  "plugin": "super-slides",
  "version": "0.5.2",
  "wired": ["trigger `daily-deck`", "credential `slides-api-key`", "entry in config/apis.json"],
  "choices": ["decks land in Drive, not locally", "weekday mornings only"],
  "skipped": ["the Slack notifier: they do not use Slack"]
})
```

Two rules about what goes in it:

- **Name a credential, never quote one.** The payload is as readable as any message, which is the whole reason `request_credential` exists.
- **`choices` and `skipped` are what earn their place.** Anything on disk can be observed next time, so a record that only lists files says nothing § 1 could not work out for itself. What the user decided, and what they turned down, is recoverable from nowhere else.
