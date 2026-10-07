---
name: Plugin Setup
description: Use when a thread asks to set up a newly installed plugin, or to set up one again after an update. Covers reusing an earlier run, finding the author's instructions, planning the steps, doing the wiring, and asking the user for what only they can provide.
---

# Plugin Setup

This thread finishes setting up a plugin that was **just installed or updated** in this workspace. The opening request names the plugin and says which. "Set up the newly installed Super Slides plugin" is a first install. "Set up Super Slides again" is an update whose setup instructions changed. Your job is to carry out the author's setup instructions with the user, who is reading this thread now.

## 1. On an update, start from what the last run already did

**Skip this section on a first install.** There is nothing to reuse, so start at § 2.

On an update, the user has been here before. Do not ask them everything again. Work out what is new first:

- **Diff the author's two setup texts.** Query `PluginInstalled` for this plugin and take the newest two. Each `setup` value sits at `payload.data.manifest.manifest.setup` (see § 2). What the author changed is the work. The rest is done.
- **Read the record of the last run.** Query `PluginSetupCompleted` and take the newest whose `plugin` matches. It holds the user's choices and what they skipped, which the author's text cannot recover.
- **Fall back to the earlier thread only when there is no record.** The previous `PluginInstalled` event holds its setup thread's id at `payload.data.manifest.setup_thread_id`, one level above the raw manifest. `query_events` reads that thread by `thread_id`. It is a whole transcript, so read it last, and only for questions the diff leaves open.

### Verify before you skip

**A record is a hint, never authority.** Skip a step only after you check that what it creates already exists:

| Step would create | Check it with |
|---|---|
| a trigger | `lucidos triggers list` |
| a webhook | `lucidos webhooks list` |
| a credential | `run_bash`, looking for a `CRED_<NAME>` entry in `env`, never printing it |
| a config entry | read the file, e.g. `data/config/apis.json` |

The plugin may have been uninstalled and reinstalled, or the user may have deleted a trigger. A plugin set up before records existed has no record at all. When the record says "done" and the workspace disagrees, the workspace is right.

Then tell the user in one line what is still in place, and ask only about what changed: two cards, not the whole interview.

## 2. Reference the author's setup instructions

The author's instructions are **not** in this thread's opening message. They live in the durable `PluginInstalled` event for the plugin:

- Call the `events` tool with `action=query`, `event_type=PluginInstalled` (newest first). The newest one is the version you are setting up.
- The author's `setup` text and the plugin `name`/`version` live at **`payload.data.manifest.manifest`**. The `events` tool wraps the stored event as `{ type, data }`. The raw manifest sits one level below the event's own `manifest` map. The plugin id sits at **`payload.data.id`**. Match the manifest's `name` to the plugin in this thread's request, then read its `setup` value. That text holds the author's steps, written as instructions about what to do with the user.

The event is immutable, so you can re-query it on any later turn.

**A `condition` names a shorter path than the one above.** The paths here *read* a stored row, envelope and all. A trigger's `on` entry or an `await_event` matches the payload with the envelope already unwrapped. So the same two values are `id` and `manifest.manifest.version`, with no `payload.data.` in front. A read path in a condition silently resolves to nothing. The engine warns at subscription time and names the real path, but get it right first.

## 3. Plan the steps as a todo list

Before you start, turn the author's instructions into a `todo_write` list, one item per concrete step. The user then sees your plan and watches it progress across turns. Flip each item to `in_progress` as you start it and `completed` as you finish it. Keep at most one item `in_progress` at a time.

## 4. Do the wiring you can; ask for what you can't

- **Do it yourself** wherever you can: writing a config file, pasting an `apis.json` snippet for a signer plugin, creating a trigger.
- **Ask the user directly** for what only they can provide: credentials, account choices, confirmations. Prefer `ask_user_question` for choices. Use `request_credential` for secrets, never chat.

### Webhooks: the plugin cannot ship one, so you create it here

A plugin ships files. A webhook is a row in the `webhooks` table, so it never travels in the bundle. When the plugin ships a trigger that subscribes to an event a third party sends, you create the hook.

That trigger is already live: install auto-registers it, subscribed to an event type nothing emits until you finish. Nothing warns anyone if you stop halfway, so treat the hook as the step that makes the plugin work.

Work in this order:

1. **Get the shared secret.** Use `request_credential` with `auth_type: "secret"`, never chat. This type signs requests and is never sent, so it takes no base URL. The sender generates the value or the user invents one. Both sides must hold the same value.
2. **Create the hook.** Run `lucidos webhooks create --name "<plugin> <sender>" --event-type <TheEventTheTriggerSubscribesTo> --hmac '{...}'`, with `credential` naming the credential you just saved. `system-knowhow/lucidos-cli` § Webhooks carries the exact `--hmac` shape per sender, plus the header allow-list. Take the event type from the plugin's `trigger.toml`, not from the author's prose. A subscription matches the string exactly, so a near miss fires nothing.
3. **Read the shipped trigger's condition against the delivery shape.** A delivery is always `{summary, headers, payload}`, with the sender's own body under `payload`. So a condition reads `payload.action`, never a bare `action`, and a header reads `headers.X-GitHub-Event`. Authors who copy the sender's API docs get this wrong, and it fails silently: the hook delivers, the trigger never fires. Fix the `trigger.toml` before you report done.
4. **Read back the URL.** `lucidos webhooks list` prints the delivery path. The full address is `{host}:{hook_port}/<workspace-slug>/<webhook-id>`. The id is minted at create time, so the sender's URL does not exist before step 2.
5. **Check the hook socket is reachable.** Deliveries land on the gateway hook socket, not the main surface. With no remote access set up, nothing outside can reach it and every delivery fails at the sender. See `system-knowhow/remote-access` for exposing that one port.
6. **Hand the user the sender-side steps.** Only they can paste the URL into GitHub, Stripe or whatever sends the event, under an account you cannot reach. Give the URL in a `<copy>` tag, with the content type, the secret field's name, and which events to subscribe to.
7. **Verify before you report done.** `lucidos webhooks list` must show the hook, and the plugin's trigger must name the same event type. State both. An unverified hook leaves a silent trigger behind.

An unsigned hook prints a bearer token once, at create time, because the engine stores only the digest. Pass it to the user the same way, and tell them it cannot be read back.

## 5. Communicate in-thread, no notifications

The user is watching this thread, so talk to them in your replies here. Do **NOT** call `send_notification`: a toast or push for steps they are already reading is noise.

## 6. Confirm when done

When every step is complete, confirm briefly what is ready to use. If the plugin ships an app, give a **clickable app link**: a markdown link with the `app:<id>` scheme, e.g. `[Super Slides](app:super-slides)`. The `<id>` is the app's folder name under `data/apps/`. A bare app name in prose is not a link. Also mention the trigger that will run, or whatever else the setup enabled.

**Finishing this thread is what flips the plugin's card.** Its button reads **Setup** while the thread runs or waits for the user's answer, and **Open** once it does neither. So a thread parked on an unanswered question keeps the card on Setup for good. Ask only what you need, then finish.

## 7. Record what you set up

The next update spawns another setup thread, and § 1 needs this record to avoid re-asking everything. Emit it before you finish:

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

Two rules for what goes in it:

- **Name a credential, never quote one.** The payload is as readable as any message, which is why `request_credential` exists.
- **`choices` and `skipped` are what earn their place.** The next run can observe anything on disk, so a list of files tells § 1 nothing new. What the user decided and turned down lives nowhere else.
