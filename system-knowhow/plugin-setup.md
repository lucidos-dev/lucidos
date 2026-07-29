---
name: Plugin Setup
description: Use when a thread asks to "set up the newly installed <name> plugin" (a plugin setup thread spawned right after an install). Covers how to find the plugin author's setup instructions, plan the steps as a todo list, do the wiring, ask the user for what only they can provide, and confirm when done.
---

# Plugin Setup

This thread exists to finish setting up a plugin that was **just installed** in this workspace. The opening request names the plugin (e.g. "Set up the newly installed Super Slides plugin"). The plugin author left setup instructions — your job is to carry them out with the user, who is reading this thread right now.

## 1. Reference the author's setup instructions

The author's instructions are **not** in this thread's opening message — they live in the durable `PluginInstalled` event for the plugin. Retrieve them:

- Call the `events` tool with `action=query`, `event_type=PluginInstalled` (newest first). The plugin was just installed, so it is the most recent one.
- In each returned event, the plugin's manifest is nested inside `payload`. The author's `setup` text and the plugin `name`/`version` live at **`payload.data.manifest.manifest`** — the `events` tool wraps the stored event as `{ type, data }`, and the raw manifest sits one level below the event's own `manifest` map. Match that manifest's `name` to the plugin named in this thread's request, then read its `setup` value. That text is the author's setup steps, written as instructions about what to do with the user.

You can re-query this event on any later turn if you need the instructions again — it is immutable, so it never goes stale.

## 2. Plan the steps as a todo list

Turn the author's instructions into a `todo_write` list (one item per concrete step) before you start. This shows the user your plan, lets them watch progress tick along, and keeps the plan visible across turns. Flip each item to `in_progress` as you start it and `completed` as you finish — at most one `in_progress` at a time.

## 3. Do the wiring you can; ask for what you can't

- **Do it yourself** wherever the step is something you can carry out (writing a config file, pasting an `apis.json` snippet for a signer plugin, creating a trigger, etc.).
- **Ask the user directly** for anything only they can provide — credentials, account choices, confirmations. Prefer `ask_user_question` for choice-shaped questions; use `request_credential` (never chat) for secrets.

## 4. Communicate in-thread — no notifications

The user is watching this thread. Communicate entirely through your normal replies here. Do **NOT** call `send_notification` — a toast or push for setup steps they are already reading is noise.

## 5. Confirm when done

When every step is complete, give a short confirmation of what is now ready to use. When the plugin ships an app, give it as a **clickable app link** so the user can open it in one click straight from this thread — a markdown link using the `app:<id>` scheme, e.g. `[Super Slides](app:super-slides)` (the `<id>` is the app's folder name under `data/apps/`). A bare prose mention of the app name is not a link. Also mention the trigger that will run or whatever else the setup enabled.
