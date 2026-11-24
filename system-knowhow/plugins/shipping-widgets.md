---
name: Plugins, Shipping widgets
description: Part of Plugins. Its index lists the sections it covers.
---

<!-- domain-event: PluginSetupCompleted: Plugin setup emits this domain event after completing its procedure. -->

One part of `system-knowhow/plugins`, which lists every part.

## Shipping widgets

A plugin may ship *reusable widgets* (ADR 0414). Such a widget's plugin owns it, not a thread. Its `apps/<id>/manifest.json` must say so:

```json
{
  "name": "Habit board",
  "description": "Today's habits, one tap each.",
  "icon": "assets/icon.svg",
  "kind": "widget",
  "reusable": true,
  "origin_plugin_id": "habit-tracker",
  "sizes": ["medium", "small"]
}
```

- **`"reusable": true`** and **`"origin_plugin_id"` equal to the plugin's `id`**, with **no `origin_thread_id`**. Any other widget folder fails validation: the scan names the folder in the catalog's errors, and install refuses with `apps/<id>/ is not a widget a plugin can ship`.
- **Any thread may embed it, and either dashboard may hold it**, and the agent sees it with the other reusable widgets.
- **It offers no "Stop reusing"**, and `widgets(action="stop_reusing")` refuses it. Uninstall removes it with the rest of the plugin, and an update replaces it. Deleting a thread never touches it.
- **Make one from a thread's widget** by copying its folder into the plugin's `apps/` and rewriting the manifest as above: drop `origin_thread_id`, add `origin_plugin_id`, set `reusable`.
- **Draw it per size.** A plugin widget draws into a *widget box* in a thread card and a dashboard tile alike. One written for content height clips in both. Follow "The widget box" in `system-knowhow/building-an-app/when-an-app-is-the-right-answer.md` § Widgets.
- **Narrow its sizes** with `"sizes"`, default first, only if it cannot draw all three (none declared means all three, starting medium). Set `"transparent": true` for one that draws its own shape (`system-knowhow/building-an-app/when-an-app-is-the-right-answer.md` § Widgets). A bad `sizes` fails validation like a bad origin. The user adds it from a *dashboard*'s own Add, and it runs no code until the plugin is installed: Add lists only installed widgets.
- **A plugin widget brings work in.** It reads the user's own data through `lucidos.*`, and a tap leads into a thread or an app with `lucidos.ui.navigate`. It never frames another product's interface so the user works there instead: that would be a room, not a ramp (`docs/philosophy.md`).

## Artifact provenance

Plugin update backups that the user confirms are person-authored artifact writes.
An upstream patch prepared before its proposal thread exists is engine-authored.
Neither is an unattended trigger write, so both remain eligible for workspace memory.
Artifact provenance changes no plugin manifest fields or installation confirmation requirements.

## Install confirmation panel

`plugins(action="install")` (and `plugins(action="update")`) never write directly to `data/`. The engine fetches and validates into a staged temp dir, and checks the manifest's `engine` requirement. A plugin this release cannot take is refused there, and no panel opens. Otherwise the engine opens a confirmation panel in the same content pane as a credential request. The panel shows:

- The plugin name + version + description from the manifest, under **Update plugin** when the plugin is already installed and **Install plugin** otherwise. The staged payload carries `is_update`, read from the plugin's install record rather than from overwritten files
- The `source` (git URL or archive path) and `source_type` (git / archive)
- When the manifest has no `engine`, the note "This plugin doesn't say which Lucidos version it needs." The staged payload carries `engine_requirement` (as authored, or `null` when undeclared)
- Every `data/`-relative path the install will write (overwrites called out separately in yellow)
- On an update, **Files to remove**: every path the previous version shipped and this one dropped. The staged payload carries them as `removes`. An edited one is listed under **Your edits** instead (see "An update keeps your changes")
- On an update, **Triggers that use these files**: every *affected trigger*, open by default. Each row names the trigger, the removed path, and why it matched. The staged payload carries them as `affected_triggers` (see "Affected triggers")
- The `setup` field rendered as markdown, if present. On an update it adds a line saying whether setup runs again after this update, and why. The staged payload carries `setup_run`: `null` when confirming starts no setup thread, otherwise the occasion it would seed

The user clicks **Install** (or **Update**) or **Cancel**. Until they click, no bytes hit `data/`.

- **Install** or **Update** writes the files and **commits them to the workspace git repo in one commit**, `"Install plugin: <id> v<version>"`. An update also deletes the files under **Files to remove** in that same commit. It emits `PluginInstalled`, and reloads WASM signers if any `auth-modules/` file changed. The plugin's files are then version-controlled like `write_file`/`edit_file` writes: with history, recoverable on a hard reset, and visible to git-based backups.
- **Cancel** drops the staging and emits `PluginInstallCanceled`.

Uninstall is symmetric: confirming it deletes the recorded files and commits the deletion (`"Uninstall plugin: <id> v<version>"`) before emitting `PluginUninstalled`.

**Confirming does not close the panel: it becomes a receipt.** It turns into a read-only, timestamped record of what the engine wrote or deleted. It lists any affected triggers under **Triggers to check**, and an install receipt offers **Open app** for each app it installed. The header's back arrow leaves it. It keeps its nav-history row, relabelled "Installed <name>" / "Uninstalled <name>", so Back or a reload still shows what happened. Cancel closes the panel, since nothing happened.

**Setup runs on confirm, but only when there's *new* setup to run.** If the manifest carried a non-empty `setup` field, confirming spawns a **Lucidos Agent setup thread** and navigates the user into it.

- The thread's first message is one short line, `Set up the newly installed <name> plugin.`, **not** a wall of agent instructions.
- The agent loads `system-knowhow/plugin-setup` (the chat system prompt nudges it to). It reads your `setup` text from the durable `PluginInstalled` event, at the payload path that knowhow names.
- It fires for **both** the Plugins panel's Install button and the agent's `plugins(action="install")` tool, since they share the confirm endpoint.
- The `PluginInstalled` event records the thread's id (`manifest.setup_thread_id`), so the plugin's card can resolve it later.
- A fresh install spawns it. An **update** spawns it in two cases: the new `setup` text **differs** (after trimming) from the installed version's, or `setup_revision` is **greater** than the installed one. Otherwise it re-runs nothing and navigates nowhere.
- The background marketplace **update check** never installs, so it never spawns a setup thread.

**The setup thread's first message is the engine's, not yours.** It carries an engine origin with the reason `plugin_setup`: the plugin, the version installed, the occasion and the device that confirmed. The occasion is a fresh install, or an update with its cause and the prior version when known. The message route popover shows it as "Plugin install" or "Plugin update".

**An update's setup thread starts from the last run, not from scratch.** Its seed says so, and `system-knowhow/plugin-setup` § 1 acts on it. That section reads the `PluginSetupCompleted` record the previous run wrote, and verifies what is already wired before asking anything. So an update costs the user a couple of questions, not the whole interview. The seed is titled `Update <name> setup` and names the cause:

| Cause | Seed |
|---|---|
| The `setup` text changed | `Set up <name> again: its setup instructions changed since version <prior>.` |
| `setup_revision` was raised | `Set up <name> again: version <v> asks to run its setup again since version <prior>.` |
| Both | `Set up <name> again: its setup instructions changed since version <prior>, and the author asks to run it again.` |

A raised revision with a `setup_reason` adds ` Author's note: <reason>`. A record naming no prior version drops the `since version` clause.

What this means for plugin authors:

- **Lead with `name` and `description`.** They render at the top of the panel. A real description ("Self-healing site knowhow for browser automation. Agents emit observations during tasks…") gives the user enough to decide. A single word ("browser-skills") doesn't.
- **Use `setup` for wiring instructions the agent should run after install.** The panel lists it, folded, under "Agent setup steps", and the user can open it before confirming.

  On confirm, the setup thread's agent reads it, plans it as a todo list and walks the user through it. It asks for what it needs (credentials, choices) and does the wiring it can (e.g. pasting the `apis.json` snippet for a signer plugin). Write `setup` as instructions *to the agent about what to do with the user*, not as a static checklist.
  This is also the only way to ship anything that is workspace state rather than a file, a webhook above all.
- **Make every setup step that installs a shipped file outside `data/` safe to re-run.** A release can ask for setup to run again, so the step must check what is already there. It asks before it overwrites a local edit. A worked example, for a plugin that ships a Claude Code skill in `knowhow/<name>/skill/`:

  > Copy `data/knowhow/<name>/skill/` to `~/.claude/skills/<name>/`. If that folder already exists, compare it with the shipped copy. When they match, say so and skip the copy. When the user changed their copy, show the difference and ask before you overwrite it.

- **Updates inherit the same panel.** `plugins(action="update")` re-fetches the source and routes through the same staging path. The user reviews the new version's file list before any bytes are written: new files, files to replace, and files to remove.
- **Staged installs expire after 1 hour.** The user then has to re-call `plugins(action="install")`. Engine restarts also drop in-flight stagings (the staged temp dir is gone).
- **The panel survives a reload, but not the expiry.** An install staged from a chat is a *form request* (`PluginInstallRequested`, see `system-knowhow/thread-events/form-requests.md` § Form requests). Its row in the thread stays live until Confirm or Cancel, and `FormRequestResolved` records the outcome. On expiry the request closes as `expired` and the row stops offering Confirm. Uninstall behaves the same.

### A release can ask for setup to run again

Setup can install files that live outside `data/`. A Claude Code skill that setup copies to `~/.claude/skills/<name>/` is the usual case. A release that changes only the skill's files leaves the `setup` text unchanged, so a plain update starts no setup thread. The copy outside `data/` then goes stale without notice. `setup_revision` is the author's way to say so.

- **Raise it by one** on the release that needs the re-run. Adding the field is itself a raise: going from absent to `setup_revision = 1` re-runs setup on that update.
- **Leave it alone otherwise.** The re-run fires only when the staged revision is **greater** than the installed one. A value carried unchanged into the next release does nothing, so a stale value never nags.
- **Lowering or removing it never re-runs setup.** Rolling back a release is safe.
- **Add `setup_reason`** naming what changed, for example "The skill's trigger phrases changed." The user reads it in the panel, and the agent reads it first.
- **Never reword `setup` to force a re-run.** That turns the instructions into a version stamp, and the user then re-reads steps that did not change.
- **A fresh install ignores the revision.** It runs setup whenever `setup` exists.

A malformed revision or reason refuses staging, like a bad `engine`, and names the field. The revision must be a positive integer. The reason must be one non-empty line of at most 200 characters, and needs a revision. A revision needs a non-empty `setup`.

