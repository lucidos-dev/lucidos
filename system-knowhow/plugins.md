---
name: Plugins
description: Use when the user wants to author, package, publish, share, install, update, or uninstall a Lucidos plugin: "build a plugin", "publish a plugin", "share this app as a plugin", "package this for other workspaces", "ship a knowhow bundle", "install the X plugin", "update plugins", "make a .lucidos-plugin".
---

# Plugins

How to package a coherent bundle of workspace content (apps, knowhow, triggers, scripts) so another workspace installs it as a unit. It also covers distribution, updates and uninstall. The v1 design is `docs/plans/2026-04-29-plugins-v1-design.md`. For setup right after install, see `plugin-setup.md`.

## When a plugin is the right artifact

Use a plugin only when the bundle is a coherent thing someone else would want to install whole.

| You want to share... | Right answer |
|---|---|
| One app the user pinned and likes | App: copy the `apps/<id>/` tree. No plugin needed for a file or two. |
| A standalone knowhow file | Knowhow file: paste it into `data/knowhow/`. Knowhow is already portable. |
| An app + the knowhow it relies on + the trigger that drives it | Plugin: the pieces only make sense together. |
| A self-healing browser-skills loop (write-side knowhow + read-side reflection knowhow that a trigger calls) | Plugin. Canonical example: `lucidos-dev/plugins/browser-learning`. |
| A WASM auth signer (`<name>.wasm` + `<name>.manifest.json`) and the apis.json snippet that calls it | Plugin. Ship the signer in `auth-modules/`. Use the `setup` field to walk the user through wiring `apis.json` and credentials at install. Signer ABI: `system-knowhow/building-an-auth-handshake.md`. |
| Engine defaults every workspace should always have | Not a plugin: it belongs in `system-knowhow/`. |

The test: removing any one file leaves the others broken or misleading. If the files don't cohere, ship them separately.

## Questions to settle with the user before bundling

Other workspaces install what you publish, so get the shape right before scaffolding. Skip questions the user has already answered.

1. **What's the cohesive unit?** List the files you intend to bundle and confirm each one would be misleading or non-functional without the others. If the answer is "they're related but each works alone", ship them as separate apps / knowhow / triggers, not a plugin.
2. **`id` and `name`?** `id` must match `[a-z0-9-]+`, max 64 chars: confirm the slug. `name` is the human title.
3. **Distribution shape?** Single-repo git URL, monorepo subpath, or local `.lucidos-plugin` archive. Drives whether `source` is set in the manifest and where the user will publish.
4. **Pictures and widgets?** Propose an icon, two to four screenshots and a `media/README.md` for every plugin you package (see "Plugin media"). Ask which of the workspace's reusable widgets should ship with it (see "Shipping widgets").
5. **Cron triggers, OAuth, personal data?** None of those ship in plugins (see "What doesn't belong in a plugin" below). If the bundle would benefit from a cron trigger, surface that in the manifest `description` so the install-time LLM can offer to set one up. Confirm with the user that this is the intended UX.

## Plugin layout

A plugin is a directory with `manifest.toml` at the root plus a subset of seven content directories. They mirror `data/` one-to-one: `<plugin>/<dir>/...` lands at `<workspace>/data/<dir>/...` at install.

```
my-plugin/
  manifest.toml          # required, at root
  apps/                  # optional, mirrors data/apps/
  knowhow/               # optional, mirrors data/knowhow/
  triggers/              # optional, mirrors data/triggers/
  scripts/               # optional, mirrors data/scripts/
  auth-modules/          # optional, mirrors data/auth-modules/
                         #   ship `<name>.wasm` + optional `<name>.manifest.json` sidecar;
                         #   install auto-reloads the proxy WASM signer map
  themes/                 # optional, mirrors data/themes/
                         #   one `<id>.json` per theme; see themes.md
  fonts/                 # optional, mirrors data/fonts/
                         #   one `<slug>/` per font; see workspace-fonts.md
  media/                 # optional, never installed as content:
                         #   the icon, screenshots, videos and README.md
                         #   the Plugins panel shows; see "Plugin media"
```

The `manifest.json` sidecar carries only WASM-host metadata (`secret_handles`, `body_mode`, `capabilities`). The engine never loads provider config from it: `data/config/apis.json` is the single source of truth for proxy entries. A plugin that ships a signer puts the matching `apis.json` snippet in its `setup` field. The install-time LLM then walks the user through pasting it into `data/config/apis.json` and registering the credential.

Install validates the tree (`core/plugins.rs::validate_tree` and `validate_archive_entry_path`). Any failure rejects the archive before any file is written:

- `manifest.toml` exists at the plugin root and parses.
- All required manifest fields are present (`id`, `version`, `name`, `description`).
- `id` matches `[a-z0-9-]+`, non-empty, max 64 chars. Uppercase, underscore, dot all reject.
- `version` parses as semver.
- `source`, when present, looks like a git remote: starts with `https://`, `http://`, `git@`, or ends in `.git`. Bare strings, `file://` URLs, and `.lucidos-plugin` paths are not valid. `source` is optional: omit it for archive-only plugins shared peer-to-peer (Slack drop, USB stick, attachment). `update_plugin` and `check_plugin_updates` refuse a sourceless plugin with an explanatory error. Install and uninstall work fine.
- Top-level entries are exactly `manifest.toml` plus a subset of `{apps, knowhow, triggers, scripts, auth-modules, themes, fonts, media}`. `media/` is never content: it holds *plugin media* and counts toward nothing below. No root README, no `LICENSE`, no `.git`, no `node_modules`, no `__MACOSX` (macOS Finder adds it when zipping). Put per-plugin docs and license inside the plugin's own subtree if needed. A `looks/` folder, the name themes had before ADR 0316, is refused with a hint to rename it `themes/`.
- At least one of `apps/`, `knowhow/`, `triggers/`, `scripts/`, `auth-modules/`, `themes/`, `fonts/` exists with at least one file. An empty `knowhow/` directory passes the top-level check but fails as `EmptyTree`.
- Every file under `themes/` is `<id>.json` and a valid theme, with an id no built-in theme uses (`InvalidTheme` otherwise). The rules are in `themes.md`, theme parts included. A part value past a cap, or a part on a protected surface, fails staging with the field, the rule and the limit (`dark.parts.chat-text.text-shadow: blur 2em is over the 0.6em cap.`).
- Every file under `fonts/` belongs to a valid workspace font: `fonts/<slug>/font.json` plus the font files it names (`InvalidFont` otherwise). A theme in the plugin may name a workspace font only if the plugin ships it. Install is refused when the plugin's new fonts would take the workspace past 100. The rules are in `workspace-fonts.md`.
- The file walk silently skips hidden files (any path component starting with `.`), such as `.DS_Store` and editor swap files.
- The file walk silently skips build output. It does not enter a directory named `node_modules`, `target`, `dist`, `build`, `out`, `__pycache__`, `venv`, `.venv`, `.next`, `.pytest_cache` or `.git`, at any depth. It also skips any `*.pyc` or `*.pyo` file. These files are machine-specific, so they never reach the install record or the uninstall list. A tree that holds only build output fails as `EmptyTree`.
- An app, trigger or font folder with one of those names (`apps/dist/`, `triggers/build/`, `fonts/out/`) fails as `ItemNamedLikeBuildOutput`, so install never drops a whole item silently. Deeper down, install drops such a folder without a warning, so do not put real content in one.
- Symbolic links are never followed. A file symlink is skipped and a dir symlink is not walked. A top-level symlink named as a content dir is rejected. This keeps a plugin from reaching a file outside its own tree.
- No archive entry uses `..` or absolute paths (`/`, `\`): zip-slip protection.

There is no separate install destination. `apps/foo/index.html` in the plugin lands at `data/apps/foo/index.html` in the workspace. Sub-trees (`triggers/foo/foo.md`, `apps/foo/sdk-prefs.js`) are preserved verbatim.

## `manifest.toml` schema

Four required fields, seven optional. Unknown extra fields are accepted and round-trip into the `PluginInstalled` event payload's `manifest`, so future additive fields stay compatible with old install records.

| Field | Required | Type | Notes |
|---|---|---|---|
| `id` | yes | string | `[a-z0-9-]+`, max 64 chars. Used as the install-record key, the event `aggregate_id`, and the canonical argument to `update_plugin` / `uninstall_plugin`. (`uninstall_plugin` also accepts the manifest `name` or any `apps/<dir>` folder name the plugin owns, case-insensitive. It picks one if unambiguous, otherwise lists candidates.) |
| `version` | yes | string | Semver (`MAJOR.MINOR.PATCH`). `0.1.0`, `1.4.2-beta.1` both parse. |
| `name` | yes | string | Human-friendly title shown in install/uninstall messages. |
| `description` | yes | string | One-line summary. Free text. If your plugin pairs well with a cron trigger, mention it here (e.g. "Ask Lucidos to set up a daily reflection trigger after install"). The install-time LLM then offers to wire one up: see "What doesn't belong in a plugin" below. |
| `source` | no | string | Git remote URL where the plugin lives. Used by `check_plugin_updates` and `update_plugin` to re-fetch the manifest. Omit for archive-only sharing: the plugin still installs and uninstalls, but the update tools return an explanatory error. When present, it must look like a git remote (`https://`, `http://`, `git@`, or ending in `.git`). |
| `engine` | no | string | Semver requirement on the Lucidos release (e.g. `">=0.46.1"`). **Enforced at install and update**: see "The `engine` requirement" below. Omit it and any release installs the plugin, but Lucidos shows that it declares none. Declare it. |
| `setup` | no | string | Markdown wiring instructions, written **to the agent about what to do with the user**. It renders in the install confirmation panel before the user confirms, and on confirm it drives a spawned Lucidos Agent setup thread. The engine never interprets it. This is the only way to ship workspace state rather than a file, a webhook above all. See "Install confirmation panel" below and `plugin-setup.md`. |
| `icon` | no | string | The plugin icon, a path inside `media/` (`"media/icon.svg"`). See "Plugin media". |
| `screenshots` | no | array of string | Paths inside `media/`, in the order the plugin detail page shows them. |
| `videos` | no | array of string | Paths inside `media/`, shown after the screenshots. |
| `categories` | no | array of string | Topical tags for browsing the **Store** (the Plugins panel's category filter). A **controlled vocabulary** (see below): pick from the allowed set. Normalised to lowercase on parse. An unknown value is **dropped and flagged** at catalog-scan time (it appears in the catalog's `errors`), never blocking install. |

**Plugin categories: the controlled vocabulary.** The allowed values are: `productivity`, `finance`, `health`, `developer-tools`, `data`, `communication`, `automation`, `lifestyle`, `research`, `fun` (kebab-case). The catalog offers a filter pill per category that appears in it, and each card shows its category chips. The set stays small so categories stay browsable: a free-form tag would fragment (`finance` vs `money` vs `budgeting`). Tag a plugin with the one or few that fit, or omit `categories` if none do. (Source of truth: `PLUGIN_CATEGORIES` in `crates/lucidos-engine/src/core/plugins.rs`.)

**The `engine` requirement.** Set `engine` to the **first Lucidos release that has every platform feature your plugin uses**, as a floor: `engine = ">=0.46.1"`. For example, a plugin that styles a field with the shared `.text-input` class needs 0.46.1, the first release that ships it. The engine checks it every time a plugin is staged: the `install_plugin` and `update_plugin` tools, and the Plugins panel's Install and Update buttons.

- **Met**: staging goes ahead as usual.
- **Not met**: staging is refused before anything is written and before the confirmation panel opens. The refusal names the plugin, the requirement and the running release: `Theme Studio 0.1.0 needs Lucidos 0.46.1 or later. This is Lucidos 0.46.0. Update Lucidos first.`
- **Not a valid requirement** (a typo such as `"latest"`, an empty string, or a number instead of a string): refused the same way, with the bad value quoted. A malformed requirement never installs.
- **Release unreadable**: if Lucidos cannot read its own release, it refuses any plugin that declares `engine`, since nothing can vouch for it. A plugin without `engine` still installs.
- **Not declared**: the plugin installs and updates on any release, exactly as before. Nothing is refused, disabled or asked. Lucidos only shows the gap:
  - The confirmation panel carries a quiet note under the source: "This plugin doesn't say which Lucidos version it needs."
  - The Plugins panel row carries a muted "No version requirement" chip, with the same sentence as its tooltip.

**Always declare `engine`.** Without it, an older release installs the plugin, and it breaks at run time with no clear message. For example, an app that calls `lucidos.request` needs 0.39.0 or later, the first release that ships it. Declared, the same install is refused up front, with a sentence that tells the user to update Lucidos. Your floor is the release that first shipped the newest platform feature your plugin uses.

**The workspace audit keeps asking until you declare one.** `system-knowhow/workspace-audit/plugins.md` checks every installed plugin and every plugin tree a workspace authors. It flags a plugin with no `engine` on every pass, with a fix the user can act on: update to a version that declares one, ask the author, or add the floor themselves.

**Adding `engine` later still needs a version bump.** `check_plugin_updates` compares semver. Add `engine` without raising `version`, and every existing installer sees `Already at latest` and never receives the floor.

**For a third-party plugin you cannot edit, propose the floor upstream.** Never try to add `engine` to the installed copy. `manifest.toml` never lands under `data/`, and the floor is read from the fetched source at install and update time. Use "Proposing your patch upstream" below, or a plain issue or pull request against the plugin's repository.

The running release is the one Lucidos reports as `release` on `/api/v1/health`. A development build made after a release counts as the next patch, so a dev build of 0.46.2 meets `>=0.46.2`. Pre-release and build suffixes are ignored when comparing. The requirement is checked at install and update time only: an installed plugin is never removed or blocked because of it. A floor (`>=X.Y.Z`) reads as "X.Y.Z or later" in every message. Any other shape, such as `">=0.40, <0.46"`, is quoted as written.

Worked example (`browser-learning/manifest.toml`):

```toml
id = "browser-learning"
version = "0.1.0"
name = "Browser Learning"
description = "Self-healing site knowhow for browser automation. Agents emit observations during tasks; a reflection recipe folds them into per-domain knowhow so the next agent visits with better priors."
source = "https://github.com/lucidos-dev/plugins/tree/main/browser-learning"
engine = ">=0.9.5"
categories = ["automation", "developer-tools"]
```

It ships knowhow only, so its floor is the first release with a working plugin install/uninstall lifecycle, not a later UI feature.

The `source` may be the GitHub tree URL the user copied from the address bar. The install tool parses it back into a git remote, branch and subpath. For a single-repo plugin, use the bare git URL.

## Plugin media

*Plugin media* is what the Plugins panel shows for a plugin: an icon, screenshots, videos and a long description. It lives in the plugin's top-level `media/` folder, and the manifest names each file. Lucidos never merges media into the workspace's content, and runs no plugin code to show it (ADR 0414).

```toml
icon = "media/icon.svg"
screenshots = ["media/board.png", "media/settings.png"]
videos = ["media/tour.mp4"]
```

```
my-plugin/
  manifest.toml
  apps/...
  media/
    icon.svg
    board.png
    settings.png
    tour.mp4
    README.md            # the long description, no manifest key needed
```

| Entry | Formats | Limit |
|---|---|---|
| `icon` | SVG, PNG, WebP | 512 KB, the app icon's limit |
| each of `screenshots` | PNG, JPEG, WebP | 4 MB, at most 8 |
| each of `videos` | MP4, WebM | 25 MB, at most 3 |
| `media/README.md` | markdown | 256 KB |
| everything together | | 64 MB |

- **Paths start with `media/`.** Lucidos drops a path outside it, a symlink, a missing file, a wrong format and a file over its limit. It also drops an entry past a count limit, or one that takes the total over 64 MB.
- **A media problem never blocks.** The scan lists the plugin and install goes ahead. The plugin detail page names each dropped file and the limit it broke, so the author sees it in their own Plugins panel.
- **No `icon`?** The Plugins panel falls back to the app icon of the plugin's single app (widgets do not count), then to a *monogram tile*.
- **`media/README.md` is the long description.** It renders as markdown on the plugin detail page, through the same sanitizer as chat, so raw HTML and scripts are stripped. The manifest `description` stays the one line the list row shows.
- **Where it is kept.** A marketplace scan keeps each listed version's media in `.lucidos/plugin-media/`, a cache the next scan rebuilds. Install copies the plugin's media to `data/plugin-media/<id>/`. It is engine-managed and gitignored, never in a commit, the file list or the "Modified" badge. It outlives the marketplace and is in every backup. An update replaces it whole, and uninstall deletes it.
- **Served by `GET /api/v1/plugins/media/...`**, the Lucidos shell only, never an app. It serves only what a kept copy lists: the manifest's entries, the README and the app icon fallback. Every answer carries `X-Content-Type-Options: nosniff` and `Content-Security-Policy: sandbox; default-src 'none'`. An SVG opened directly cannot run script.

## Shipping widgets

A plugin may ship *reusable widgets* (ADR 0414). Such a widget's plugin owns it, not a thread. Its `apps/<id>/manifest.json` must say so:

```json
{
  "name": "Habit board",
  "description": "Today's habits, one tap each.",
  "icon": "assets/icon.svg",
  "kind": "widget",
  "reusable": true,
  "origin_plugin_id": "habit-tracker"
}
```

- **`"reusable": true`** and **`"origin_plugin_id"` equal to the plugin's `id`**, with **no `origin_thread_id`**. Any other widget folder fails validation: the scan names the folder in the catalog's errors, and install refuses with `apps/<id>/ is not a widget a plugin can ship`.
- **Any thread may show it**, and the agent sees it with the other reusable widgets.
- **It offers no "Stop reusing"**, and `widgets(action="stop_reusing")` refuses it. Uninstall removes it with the rest of the plugin, and an update replaces it. Deleting a thread never touches it.
- **Make one from a thread's widget** by copying its folder into the plugin's `apps/` and rewriting the manifest as above: drop `origin_thread_id`, add `origin_plugin_id`, set `reusable`.

## Install confirmation panel

`install_plugin` (and `update_plugin`) never write directly to `data/`. The engine fetches and validates into a staged temp dir, and checks the manifest's `engine` requirement. A plugin this release cannot take is refused there, and no panel opens. Otherwise the engine opens a confirmation panel in the same content pane as a credential request. The panel shows:

- The plugin name + version + description from the manifest
- The `source` (git URL or archive path) and `source_type` (git / archive)
- When the manifest has no `engine`, the note "This plugin doesn't say which Lucidos version it needs." The staged payload carries `engine_requirement` (as authored, or `null` when undeclared)
- Every `data/`-relative path the install will write (overwrites called out separately in yellow)
- The `setup` field rendered as markdown, if present

The user clicks **Confirm** or **Cancel**. Until they click, no bytes hit `data/`.

- **Confirm** writes the files and **commits them to the workspace git repo in one commit**, `"Install plugin: <id> v<version>"`. It emits `PluginInstalled`, and reloads WASM signers if any `auth-modules/` file changed. The plugin's files are then version-controlled like `write_file`/`edit_file` writes: with history, recoverable on a hard reset, and visible to git-based backups.
- **Cancel** drops the staging and emits `PluginInstallCanceled`.

Uninstall is symmetric: confirming it deletes the recorded files and commits the deletion (`"Uninstall plugin: <id> v<version>"`) before emitting `PluginUninstalled`.

**Confirming does not close the panel: it becomes a receipt.** It turns into a read-only record of what the engine wrote or deleted, with a timestamp and no buttons. The header's back arrow leaves it. It keeps its nav-history row, relabelled "Installed <name>" / "Uninstalled <name>", so Back or a reload still shows what happened. Cancel closes the panel, since nothing happened.

**Setup runs on confirm, but only when there's *new* setup to run.** If the manifest carried a non-empty `setup` field, confirming spawns a **Lucidos Agent setup thread** and navigates the user into it.

- The thread's first message is one short line, `Set up the newly installed <name> plugin.`, **not** a wall of agent instructions.
- The agent loads `system-knowhow/plugin-setup` (the chat system prompt nudges it to). It reads your `setup` text from the durable `PluginInstalled` event, at the payload path that knowhow names.
- It fires for **both** the Plugins panel's Install button and the agent's `install_plugin` tool, since they share the confirm endpoint.
- The `PluginInstalled` event records the thread's id (`manifest.setup_thread_id`), so the plugin's card can resolve it later.
- A fresh install spawns it. An **update** spawns it only when the new `setup` text **differs** (after trimming) from the installed version's. Otherwise it re-runs nothing and navigates nowhere.
- The background marketplace **update check** never installs, so it never spawns a setup thread.

**The setup thread's first message is the engine's, not yours.** It carries an engine origin with the reason `plugin_setup`: the plugin, the version installed, the occasion (fresh install or update, with the prior version when known) and the device that confirmed. The message route popover shows it as "Plugin install" or "Plugin update".

**An update's setup thread starts from the last run, not from scratch.** Its seed says so (`Set up <name> again: its setup instructions changed since version <prior>.`, titled `Update <name> setup`), and `system-knowhow/plugin-setup` § 1 acts on it. That section diffs your two `setup` texts, reads the `PluginSetupCompleted` record the previous run wrote, and verifies what is already wired before asking anything. So a reworded `setup` costs the user a couple of questions, not the whole interview.

What this means for plugin authors:

- **Lead with `name` and `description`.** They render at the top of the panel. A real description ("Self-healing site knowhow for browser automation. Agents emit observations during tasks…") gives the user enough to decide. A single word ("browser-skills") doesn't.
- **Use `setup` for wiring instructions the agent should run after install.** The user sees it as markdown before confirming. On confirm, the setup thread's agent reads it, plans it as a todo list and walks the user through it. It asks for what it needs (credentials, choices) and does the wiring it can (e.g. pasting the `apis.json` snippet for a signer plugin). Write `setup` as instructions *to the agent about what to do with the user*, not as a static checklist.
  This is also the only way to ship anything that is workspace state rather than a file, a webhook above all.
- **Updates inherit the same panel.** `update_plugin` re-fetches the source and routes through the same staging path. The user reviews the new version's file list (new files, and "would overwrite" for changed ones) before any bytes are written.
- **Staged installs expire after 1 hour.** The user then has to re-call `install_plugin`. Engine restarts also drop in-flight stagings (the staged temp dir is gone).
- **The panel survives a reload, but not the expiry.** An install staged from a chat is a *form request* (`PluginInstallRequested`, see `system-knowhow/thread-events.md` § Form requests). Its row in the thread stays live until Confirm or Cancel, and `FormRequestResolved` records the outcome. On expiry the request closes as `expired` and the row stops offering Confirm. Uninstall behaves the same.

## The Plugins panel and marketplaces

Users find and install plugins, and the *apps* they ship, in the **Plugins panel**. It is one list with an **Installed only** filter, checked by default:

- **Checked**, it lists every plugin on disk, *whatever it ships*. It reads the `PluginInstalled` projection via `GET /api/v1/plugins/installed`, so it works offline and still lists a plugin whose marketplace was removed.
- **Unchecked**, it widens to the full catalog: installed plus available from registered marketplaces.

The installed view is the home for plugins that ship **no app** (knowhow-, trigger-, script- or auth-module-only bundles). Each row shows the plugin's content kinds, its shipped files (each links to a preview) and an **Uninstall** button. When a registered marketplace offers a newer version, the row also shows an **Update available** chip and an **Update** button. The match is by plugin *id* (not `app_id`), so it works for app-less plugins too. Clicking stages the same confirmation panel as any install. A plugin's *app* still lives in the separate **Apps** panel, with its own **Update** shortcut, but the plugin itself is managed here.

A **marketplace** is a git repository (or GitHub tree URL) registered at `data/config/plugin-marketplaces.json`, added and removed under **Settings → Marketplaces**. A scan clones each one, finds valid `manifest.toml` plugin roots, and compares each version against installed `PluginInstalled` events. The catalog shows each plugin as a card, read from the last scan (see "A scan never runs on a page open" below). `Install`/`Update` stage the same confirmation panel described above. The catalog never writes plugin files directly.

**Settings → Marketplaces** adds, renames and removes a marketplace. Only the name is editable there. The id hashes the URL, so a row's URL is read-only: pointing at another repository is a remove plus an add. The rename re-posts the stored URL, which is why the registry entry keeps its id.

Each card leads with the plugin icon: its *plugin media* icon, its single app's icon, or a *monogram tile*. It shows the manifest `description` cut to two lines. A tap on the card opens its **plugin detail page** in the content pane. The page holds:

- the full description;
- every screenshot and video in one strip that scrolls sideways, where a screenshot opens full size;
- `media/README.md`, rendered as markdown;
- a muted note naming each media file the engine left out, and why.

The page carries the card's own buttons. It shows media only: no plugin code runs there. The README renders through the chat sanitizer. Lucidos drops its images, inline styles and form controls, since each would fetch from a host the author picked or draw over the page. List pictures as `screenshots` instead. Its web links open in a new tab, and a relative link shows as plain text.

Each card has a primary button that progresses **Install → Setup → Open**, plus an **Uninstall** button once the plugin is on disk:

- **Install** (or **Update** for an out-of-date install): stages the confirmation panel. A plugin whose `engine` requirement this release misses still lists. Its button is disabled, and a warning chip gives the reason ("Needs Lucidos 0.46.1 or later"). The catalog row carries `engine_requirement` (as authored), `engine_compatible` and `engine_incompatible_reason`. They are filled each time the catalog is served, so an engine upgrade counts at once.
  - A plugin with no `engine` keeps a live button and shows a muted "No version requirement" chip instead. Its row has no `engine_requirement`.
  - An installed plugin whose marketplace is gone reads the same field from `GET /api/v1/plugins/installed`, which reports the installed version's requirement.
  - The two chips never show together, because an incompatible plugin always declares `engine`.
- **Setup**: shown after install while the setup thread is still running or waiting on the user. Clicking opens that thread. Driven by `setup_thread_id` + `setup_complete` on the catalog row. Plugins with no `setup` field skip past this. The engine resolves `setup_complete` from what it can observe about the thread:
  - **present** (has a `thread_summaries` row): done once its lifecycle status is neither `running` nor `waiting_for_user_answer`.
  - **pending** (no row yet but a live `thread_queue` entry, the brief window before the agent's first event): not done, so the card keeps showing Setup without flicker.
  - **gone** (no row and no queue entry: a lost spawn, deleted thread, or stale catalog id): treated as done. The card falls through to Open/Installed rather than offering a Setup button that would 404.
- **Open**: shown once setup is finished (or the plugin had none). It launches the plugin's primary app (`data/apps/<id>/`). Plugins that ship no app show a disabled **Installed** instead.
- **Uninstall**: stages the uninstall confirmation panel (the same one the `uninstall_plugin` LLM tool produces). The card re-fetches the catalog on every mount, so the Setup→Open transition shows when the user returns from setup.

**Plugin uninstall is the single removal authority for a plugin's app.** The Apps panel's **Delete** button would `rm -rf` only `apps/<id>/`, leaving the plugin registered and its sibling `triggers/`/`knowhow/`/`scripts/` orphaned. So `DELETE /api/v1/app?id=...` returns **409** with `{ error, plugin_id, plugin_name }` when the app belongs to an installed plugin. That mirrors the `delete_file` guard, which refuses raw deletes of plugin-owned files. The UI catches the 409 and routes the user to the plugin **Uninstall** panel, which removes the whole plugin tree and emits `PluginUninstalled`. Standalone apps (no `PluginInstalled` record) keep deleting directly.

Installed marketplace plugins are **not** auto-updated: the engine notifies, and the user decides. It scans registered marketplaces at startup, after a marketplace is registered or renamed, and every five minutes. A newer version of an installed plugin produces one deduplicated `NotificationCreated` ("Plugin update(s) available") and installs nothing. Its tap opens the Plugins panel's installed list, with the **Installed only** filter on:

- **One update**: the list scrolls to and pulse-highlights that plugin, carried as the navigate `id`.
- **Several updates**: no row is highlighted, since each pending row already shows its update chip.

The user applies an update from any of three places, and each stages the same confirmation panel as any install:

- the plugin row's **Update** button, which works for *any* plugin, app or not;
- the catalog card, with **Installed only** unchecked;
- for a plugin that ships an app, the app row's **Update** button on the Apps panel.

**No notification for an update this release cannot install.** When the running release misses a version's `engine` requirement, nothing is sent. The Plugins panel still shows it, with Update disabled and the reason beside it. On the Apps panel, the app row swaps its Update button and "Update available" chip for the reason. Once Lucidos meets the requirement, that version counts as new and the notification fires.

A `.lucidos/plugin-update-notice.json` marker tracks what the user was told. The re-scan re-notifies only for a *new* update (a fresh plugin or a bumped version), not every cycle.

**A scan never runs on a page open.** `GET /api/v1/plugins/catalog` reads the *plugin catalog cache* (`.lucidos/plugin-catalog.json`), which the five-minute scan writes, so the Plugins panel and Settings → Marketplaces paint at once. The response carries `scanned_at`, `scanning` and `scan_error` beside the rows. The panel shows no age, but names the reason above the list when the last scan failed. A cache older than five minutes, or none, starts a background scan, and the request still answers at once. `POST /api/v1/plugins/catalog/rescan` queues a scan now and returns at once.

The marketplace list itself is read live from the registry on every request. So a rename shows with no scan behind it, and a removed marketplace never contributes cached plugins. A scan announces both ends as **transient** events (SSE only, never stored): `PluginCatalogScanStarted` when it begins and `PluginCatalogScanned` when it lands, the latter carrying `failed`. They raise and lower the panel's scanning state on every connected client, the scheduler's own pass included.

Every marketplace mutation is **announced**, so an open Plugins panel and Settings → Marketplaces update in place with no reload. Registering a marketplace (or re-registering one under a new name) emits `PluginMarketplaceRegistered`. Unregistering emits `PluginMarketplaceRemoved`. The frontend re-reads the catalog on either. `Registered` is an upsert covering rename as well as create: the id hashes the canonical source, so re-registering a listed source rewrites its entry in place. The announcement lives in the one shared registry write path, so it fires for the HTTP endpoints below and the `register_plugin_marketplace` tool alike.

Marketplace HTTP surface:

- `GET /api/v1/plugins/marketplaces` -> registered marketplace list.
- `POST /api/v1/plugins/marketplaces` with `{ "source": "...", "name"?: "..." }` -> register or rename a marketplace.
- `DELETE /api/v1/plugins/marketplaces/{id}` -> unregister a marketplace.
- `GET /api/v1/plugins/catalog` -> `{ marketplaces, plugins, errors, scanned_at, scanning, scan_error }`. `marketplaces` is live from the registry; `plugins` and `errors` come from the cache. Each installed plugin row also carries `setup_thread_id`, `setup_complete`, and `app_id` to drive the card's Install→Setup→Open button.
- `POST /api/v1/plugins/catalog/rescan` -> queue a marketplace scan. Answers `{ "queued": true }` at once; the result arrives as `PluginCatalogScanned`. A scan already running goes round once more after it, so the result comes from a scan that started after the request.
- `GET /api/v1/plugins/installed` -> `{ plugins }` from the `PluginInstalled` projection (no marketplace scan). Each row carries `id`, `name`, `version`, `source?`, `app_id?`, `content` (the shipped content-dir kinds), `files` (every installed `data/`-relative path), and `modified` + `modified_paths` (see "Local modifications" below). Backs the Plugins panel's installed-plugins view (the default **Installed only** filter) so it works offline and lists plugins whose marketplace was removed.
- `POST /api/v1/plugins/install-request` with `{ "source": "..." }` -> stage an install request payload for the existing confirmation panel.
- `POST /api/v1/plugins/uninstall-request` with `{ "id": "..." }` -> stage an uninstall request payload (resolves the plugin id, partitions its files into present/missing) for the uninstall confirmation panel. The button counterpart of the `uninstall_plugin` LLM tool.
- `POST /api/v1/plugins/install/:install_id/confirm[?keep_local_changes=false]` -> write the staged files. The optional flag is the panel's keep control; absent means keep. The response adds `local_changes` (`merged` / `conflicted` / `replaced` / `saved_paths`) whenever the install met a locally-edited file.
- `POST /api/v1/plugins/propose-upstream` with `{ "id": "..." }` -> `{ patch_path, thread_id }`. Derives the plugin's local patch, writes it under `data/artifacts/`, and spawns a thread to take it to the author. See "Proposing your patch upstream" below.

Marketplace LLM surface:

- `register_plugin_marketplace(source, name?)` registers or renames a marketplace in the registry the Plugins panel browses. It commits `data/config/plugin-marketplaces.json` and announces the change, so any open panel refreshes. It then starts the scan / update-check pass, which notifies about plugin updates rather than applying them. Use it when a user asks to add a plugin repo, marketplace, or plugin marketplace source. There is no unregister tool: remove a marketplace from Settings → Marketplaces.

For GitHub monorepo marketplaces, register either the repo URL (`https://github.com/lucidos-dev/plugins`) or a tree URL (`https://github.com/lucidos-dev/plugins/tree/main/community`). The scanner turns discovered subdirectory plugins into installable GitHub tree URLs. For non-GitHub monorepos, use one repo per plugin: the install tool installs a subdirectory only from a GitHub tree URL.

## Authoring a marketplace

A marketplace is not its own artifact type: there is no marketplace manifest,
no schema, no validation step. It is **a git repository whose subdirectories
contain plugin roots**. Everything above about authoring a plugin still applies.
This section covers only the repo that holds them.

### Repo layout and how the scanner finds plugins

`collect_manifest_roots` (`core/plugin_marketplaces.rs`) walks the cloned repo
looking for `manifest.toml`:

- **A directory containing `manifest.toml` is a plugin root, and the walk stops
  there.** It never descends into a plugin, so a nested `manifest.toml` inside
  an app's own tree is not mistaken for a second plugin.
- **Maximum depth is 3.** A plugin at `a/b/c/d/manifest.toml` is never found.
  Flat (`<plugin-id>/manifest.toml`) or one grouping level
  (`plugins/<plugin-id>/manifest.toml`) both work; flat is preferred because the
  generated install URL is shorter.
- **At depth 0, the seven content-dir names (`apps`, `knowhow`, `triggers`,
  `scripts`, `auth-modules`, `themes`, `fonts`) are skipped.** That guard stops
  a single-plugin repo (manifest at the root) from also reporting its own
  `apps/` as a candidate.
- **At every depth, the walk skips hidden directories and build-output
  directories** (the same list as the install walk, see "Plugin layout"). So a
  `manifest.toml` inside `node_modules/` or `dist/` is never a plugin root.
- **Duplicate plugin `id`s are de-duplicated**: the first root wins, later ones
  are silently dropped. Keep directory name == manifest `id` to make collisions
  obvious.
- A repo where the scan finds nothing fails with `no plugin manifest.toml files
  found`.

**Root-level files that are not plugin directories are ignored.** A README,
`CODEOWNERS`, `.github/`, `LICENSE`, `.gitignore` at the *marketplace* root are
fine. The strict "only `manifest.toml` + the seven content dirs" validation
applies **inside a plugin root**, not to the marketplace repo. Use the root
README as the human discovery index, as `lucidos-dev/plugins` does.

### Install URLs are generated, not authored

For a GitHub marketplace the scanner rewrites each discovered plugin root into a
tree URL (`install_source`): `https://github.com/<owner>/<repo>/tree/<branch>/<plugin-dir>`,
with the marketplace's own subpath prefixed when it was registered as a tree URL.
The branch is the one registered, else the cloned repo's actual HEAD shorthand.

Design around three consequences:

- **Subdirectory install only works for GitHub.** For a non-GitHub host, the
  fallback is the marketplace's own clone URL, which is correct only if the
  repo *is* a single plugin. Multi-plugin marketplaces on GitLab / Bitbucket /
  an enterprise host do not produce installable per-plugin URLs in v1. Use one
  repo per plugin there.
- **Renaming a plugin directory changes its install URL** and orphans the
  `source` recorded in existing installs. Treat the directory name as stable.
- Still set each plugin's own `manifest.toml` `source` to its tree URL. That is
  what `check_plugin_updates` / `update_plugin` re-fetch after install,
  independent of the marketplace.

### Registering and the scan cycle

Register the repo URL (`https://github.com/owner/repo`) or a tree URL to scope
the scan to a subdirectory (`.../tree/main/community`). `.lucidos-plugin`
archive paths are rejected: marketplaces must be git. The clone is shallow
(`depth 1`), lands in `.lucidos/tmp/plugin-marketplaces/`, and `.git` is removed
before the scan. Only the registry entry in `data/config/plugin-marketplaces.json`
persists in the workspace.

A `file://` URL is a valid marketplace source, and it clones deep rather than
shallow: libgit2's local transport rejects a shallow fetch. A local bare clone
of a repo, refreshed out of band, is a working offline marketplace.

Scans run at startup, on registration or rename, every five minutes, and on a
panel refresh (a pull, or the header's Refresh). Opening the Plugins panel only
reads the cache, and starts a scan when the cache is over five minutes old. A
scan **never installs**: it notifies about newer versions and the user clicks
Update.

### Private and internal repos

**The clone authenticates.** One shared helper, `core/git_auth.rs`, supplies
credentials for every clone the engine makes: the marketplace scan
(`clone_marketplace`), the plugin install and update fetch
(`tools/plugins/source.rs`), and both `git_clone` tool routes. A public repo is
unaffected. libgit2 asks for a credential only when the remote demands one.

**A token comes from Settings, Credentials, and nowhere else.** No environment
variable holds one. Before the clone starts, the engine looks up the credential
one of whose **Base URLs** scopes the clone URL, and hands that one credential
to the clone.

For each round libgit2 says which credential kinds the remote accepts, and the
helper offers only those, in this order:

1. **SSH agent**, for a `git@host:...` or `ssh://` URL. The key comes from a
   running ssh-agent, under the username in the URL, defaulting to `git`. When
   the URL carries no username, libgit2 asks for one first.
2. **The stored credential**, for an HTTPS URL. It travels as the password
   first, beside the username `x-access-token`, which is the form GitHub
   documents. If the host refuses that, a bare token is offered again as the
   username with an empty password, the older form some other hosts want. A
   credential that carries its own username (Basic Auth, Password) is only ever
   sent in the password form.
3. **The git credential helper**, meaning whatever `git credential` answers.
   This is the path that finds an existing `gh auth login` or a macOS keychain
   entry.
4. **No credential**, for a host that negotiates its own.

Each source is offered **at most once per clone**. libgit2 re-invokes the
callback every time the remote refuses, so offering one twice would spin
forever. Once the list is spent the clone fails with a message naming the URL
and the fix. The fix follows the URL: ssh-agent for an SSH remote, and the
credential to store for an HTTPS one. That message reaches the catalog's
`errors` array and the install tool's return, not only the log.

#### Storing the credential

Settings, Credentials, **Add**. Three fields decide the clone:

| Field | Value |
|---|---|
| Service Name | Any label you like. Nothing matches on it. |
| Base URLs | `https://github.com` for github.com. For a GitHub Enterprise install, its own host, `https://github.example.io`. |
| Auth Type | **Bearer Token**, with a token that can read the repo. |

**Base URLs are the whole scoping rule.** One must be the host you clone from, so
`https://github.com`, **not** `https://api.github.com`. Those are different
hosts, and a credential scoped only to the API host never matches a clone.

**One credential names them both.** Base URLs is a set, one row per hostname, so
one credential serves the REST API and the clone. Nothing is inferred from a
hostname's spelling, so name each host in full. Press **Add another host** in the
credential form. Or ask Lucidos for the credential again, naming the second host,
and it reopens the same row for you to save.

A Base URL may also carry a path, `https://github.com/example-org`, which scopes
it to that owner. When several credentials match one URL the longest Base URL
wins, so an org-scoped token overrides a host-wide one.

A GitHub Enterprise install needs only its own row. The clone never guesses
which host is GitHub, so there is no host list to maintain.

The row takes effect on the next scan, with no restart: every clone reads the
store when it starts.

Two alternatives need no credential row at all:

- **`gh auth login` on the machine.** It writes a helper into the git config,
  and step 3 finds it. A desktop app launched from the Dock may not have `gh`
  on its `PATH`, and then the helper cannot run.
- **An SSH URL.** Register the marketplace as `git@github.com:owner/repo.git`
  with your key in ssh-agent. Per-plugin install URLs are still rewritten to
  HTTPS tree URLs, so a multi-plugin marketplace also needs step 2 or 3 for the
  install itself.

What still does not work:

- **No token in the URL.** Lucidos saves a marketplace source verbatim in
  `data/config/plugin-marketplaces.json`, which git tracks, and shows it in the
  Plugins panel. Store a credential instead.
- **A credential is scoped by Base URL, not by repo.** A host-wide GitHub
  credential reaches every repo on github.com that the token itself can read.
  Scope the token to read access on the repos you actually need, or narrow the
  Base URL to one owner.
- **The credential must be one a git remote can present.** Bearer Token and API
  Key hold a bare token, Basic Auth and Password hold a username and password.
  An OAuth Client credential holds neither and is skipped.

### Org permissions can block repo creation independently

Creating the marketplace repo is a GitHub-side step that can fail on its own.
An org with `members_can_create_repositories: false` rejects `gh repo create`
for a plain member with
`does not have the correct permissions to execute CreateRepository`. That holds
for public, private, **and** internal alike. Check with
`gh api orgs/<org> --jq '{members_can_create_repositories, members_can_create_internal_repositories}'`
before assuming the CLI or the token is at fault.

## Shipping triggers (auto-registration)

A plugin ships a trigger by declaring it in a **`trigger.toml`** at
`triggers/<slug>/trigger.toml`, just as an app is its own folder
(`apps/<id>/manifest.json`). The file is a *trigger definition* (see
`triggers.md` § "On-disk trigger definition"): `name`, `run`
(`intent` or `script`), `on` (trigger subscriptions), and the usual optional
fields (`app_id`, `go_to_review`, `group_id`, `side_effect_grant`, and
`model` / `reasoning_effort` to pin the intent to a specific chat model). Put any
procedure the trigger needs in `triggers/<slug>/knowhow/`, beside it.

What install does (ADR 0019):

- **Auto-registers** each `trigger.toml`: emits `TriggerCreated` stamped with
  the plugin's id (provenance), so the trigger is **live immediately** (no agent
  step needed). The Triggers panel shows a "from \<plugin\>" chip on it.
- **Event-driven only.** A `trigger.toml` that declares a cron `schedule` is
  **rejected at install** (nothing is written): cron is workspace state, not
  plugin content. Ship `on:` subscriptions. For a cron cadence, see "What
  doesn't belong in a plugin".
- **Uninstall** auto-deletes exactly the triggers carrying this plugin's id
  (user-created triggers are never touched).
- **Update** re-syncs by `(plugin_id, slug)`: a still-declared slug is updated in
  place (preserving the user's paused state), a new slug is created, a dropped
  slug is removed.

The user sees the `trigger.toml` files in the install confirmation panel's file
list (with their `side_effect_grant` visible in the parsed definition) before
confirming, so activation is never silent.

## What doesn't belong in a plugin

Apps, knowhow, and **event-driven (`on_event`) triggers** belong in plugins: they are reference material or part of the plugin's own mechanism. An `on_event` trigger ships as a `triggers/<slug>/trigger.toml` declaration (see "Shipping triggers" above).

**Nothing user- or machine-specific ships in a plugin.** Many workspaces install one plugin identically. Anything that differs per installer is workspace state, not plugin content: an account, a schedule, a client id, a path on someone's disk. Ship the generic code, and use the `setup` field to have the agent ask each installer for their own value. The test for a file: would it still be correct on a machine that is not yours?

**Cron triggers, OAuth credentials, and personal data do not ship in plugins.** They are workspace state: WHEN something runs on a clock, WHO owns the account, WHAT the user has accumulated. Cron triggers stay out for four reasons:

1. **Cadence is user-specific.** Heavy users want it every 6h, light users weekly. Hardcoding `0 0 4 * * *` in the bundle makes that decision for them.
2. **The schedule is workspace state, not reference material.** A plugin shipping a cron entry is like a library shipping a crontab line: wrong layer. Knowhow is "how to do this well", cron triggers are "when I want it to happen".
3. **Orphaned cron entries.** A cron trigger the install instructions create as a side effect (an agent calling `create_trigger`) carries no plugin provenance. Uninstall does not remove it, so it is left pointing at deleted knowhow. A shipped `trigger.toml` avoids this, because uninstall deletes exactly the triggers stamped with the plugin id.
4. **Install-time prompt is the right UX.** When `install_plugin` lands the knowhow, the LLM tells the user *"This plugin works best with a reflection trigger. Want me to set one up? Daily at 4am is a good default."* Conversational, opinionated default, but the user owns the schedule.

So if a plugin would benefit from a cron trigger, mention it in the manifest `description`. The install-time LLM then offers to set one up. The canonical example is `browser-learning` v0.2.0, which ships knowhow only and relies on that prompt for its reflection (cron) trigger.

**A thread's widget does not ship.** A *widget* made in a thread names that thread (`origin_thread_id`), which no other workspace has. Ship it as a plugin widget instead (see "Shipping widgets"), or have the agent make an app from it ("Make app") and package that.

**Webhooks do not ship either, and for the same reason.** A webhook is a row in the `webhooks` table, not a file, so no manifest field or content dir holds one. Three things would have to travel with it and none can:

- The shared secret is a `credentials` row, and plugins never ship credentials.
- The delivery URL does not exist until create time, and its host is the installing machine's own funnel.
- Only the account owner can do the sender-side registration.

Everything downstream of the event still ships, and it is most of the value: the `triggers/<slug>/trigger.toml` subscribing to the pinned event, the script that reads the payload, and the knowhow describing the payload shape and its field paths. The `setup` field bridges the hook itself. Write it as instructions to the agent:

1. Request the shared secret as a credential.
2. Run `lucidos webhooks create` with the same event type the trigger subscribes to.
3. Read back the delivery path, and hand the user the URL plus the exact steps to register it with the sender.

**A plugin doing this should say so in its `description`.** The trigger goes live at install, subscribed to an event type nothing emits until setup finishes. If the user cancels the setup thread, or the create fails, the trigger never fires and nothing warns anyone. The `setup` text should verify the hook exists before it reports done.

## Where a plugin keeps its runtime state

A plugin's scripts, triggers and apps often write state when they run: a cursor, a last-seen id, a cache, a log. Put that state under `data/artifacts/<plugin-id>/`. Never write it inside the plugin's own `triggers/`, `apps/`, `scripts/` or `knowhow/` folders.

Those folders hold shipped content, so state there goes wrong two ways:

- **Updates and uninstalls manage them.** An update replaces or merges the files the plugin shipped. Uninstall deletes them. State kept among them is easy to lose.
- **The engine reads state there as a local edit.** The Modified badge counts any file added to the plugin's app folder, and any change to a file the plugin shipped. An update's three-way merge then treats that change as the user's edit. A plugin nobody touched looks modified.

Build the path from `LUCIDOS_WORKSPACE`, the workspace root the engine sets for every process it spawns:

```python
import os

STATE_DIR = os.path.join(os.environ["LUCIDOS_WORKSPACE"], "data", "artifacts", "my-plugin")
os.makedirs(STATE_DIR, exist_ok=True)
```

Write state files directly, not with `lucidos data write`. That command commits and announces every write, which suits a finished report but not a cursor rewritten on every run.

This overrides the `__file__`-relative state path in `triggers.md` § "Scripts run in place". A plugin's scripts also run in place, so that section still explains where `__file__` points. Its advice to keep state beside the script suits a trigger the user owns. For a plugin, beside the script is inside a folder the plugin manages.

## Three distribution shapes

`install_plugin(source)` detects the shape by string format (`engine/tools/plugins.rs::detect_source`).

### 1. Single-repo plugin: plain git URL

The plugin tree sits at the repo root.

```
github.com/owner/my-plugin
  manifest.toml
  knowhow/
  ...
```

Install URL: `https://github.com/owner/my-plugin` or `https://github.com/owner/my-plugin.git`. The engine shallow-clones the default branch.

Pick this when the plugin is a standalone project with its own README, issue tracker, and release cadence.

### 2. Monorepo with subpath: GitHub tree URL

Many plugins live under one repo, each in its own subdirectory.

```
github.com/lucidos-dev/plugins
  README.md                       # repo-level discovery index, not part of any plugin
  browser-learning/
    manifest.toml
    knowhow/
  habit-tracker/
    manifest.toml
    apps/
```

Install URL: `https://github.com/lucidos-dev/plugins/tree/main/browser-learning`. GitHub shows exactly this in the address bar on the plugin's directory, so install is copy and paste.

Parse rules (`parse_github_tree`): the URL must be `https://github.com/<owner>/<repo>/tree/<branch>[/<subpath>]`. The engine clones `https://github.com/<owner>/<repo>.git` at `<branch>`, then treats `<subpath>` as the plugin root. Subpath is optional (a tree URL pointing at the repo root works too).

Pick this when shipping plugins together makes sense: shared review cadence, one CI, one README listing them all. The canonical example is `lucidos-dev/plugins`. Its top-level README is a human discovery index, which the engine ignores because the subpath isolates the plugin tree.

For non-GitHub monorepos in v1, fall back to one repo per plugin: only GitHub tree URLs are parsed.

### 3. Local archive: `.lucidos-plugin` file

A `.lucidos-plugin` is a **PKZip archive** of the plugin tree, renamed. Always build with `zip`:

```
cd my-plugin
zip -r ../my-plugin.lucidos-plugin .
```

**Do not use `tar`, `tar -czf`, `gzip`, or any non-zip format.** The custom extension does not change the format. The engine opens it with `zip::ZipArchive::new()` (`engine/tools/plugins.rs::extract_zip`), which only understands PKZip. A gzipped tarball or raw gzip stream fails with an opaque "read archive: ..." parse error, and the user has to repackage. If `zip` is not installed, install it (`brew install zip`, `apt install zip`) rather than substituting another archiver.

Install URL: an absolute filesystem path ending in `.lucidos-plugin` (`/Users/me/Downloads/my-plugin.lucidos-plugin`). The engine extracts the zip into a temp dir and validates as if it were a git checkout.

Pick this for ad-hoc sharing (Slack, email), pre-publication testing, and plugins that should not go to a public git host. The custom extension makes the file self-announcing and leaves room for OS file association later. Archive plugins may omit `source`: they install and uninstall normally, but `check_plugin_updates` / `update_plugin` report nowhere to fetch from. For updates while distributing as an archive, set `source` to the git repo the archive is built from.

## Authoring loop

1. **Lay out the tree.** Create `my-plugin/manifest.toml` and the content directories. Author content as if it were already installed:
   - knowhow files use the same frontmatter rules as any other knowhow (`system-knowhow/building-knowhow.md`);
   - apps follow the app conventions (`system-knowhow/building-an-app.md`);
   - triggers obey the intent-vs-procedure rule (`system-knowhow/triggers.md`).
2. **Add the plugin media.** The Plugins panel shows it before anyone installs, so treat it as part of the plugin:
   - **Icon**: write `media/icon.svg`, a simple shape on the app's own colours. With no icon, the panel shows the single app's icon or a *monogram tile*.
   - **Screenshots**: take them yourself. Show each view worth showing, then call `capture_app` with `save_as_artifact` (`plugin-shots/board.png`), and copy the file into `media/`. The file is sharp, at the device's pixel ratio, in the format the extension names. Phone-width views read best in the detail page's strip. The capture comes from the turn's last used device, so ask the user to open the app on their phone first. Prefer `.png`, and use `.jpg` for a view over the 4 MB limit.
   - **README**: write `media/README.md` for the person deciding to install: what it does, what it needs, what to try first. Keep it to text and links, since the detail page shows no images from it.
   - **Manifest**: list each file under `icon`, `screenshots` and `videos`, within the limits in "Plugin media".
   - **Widgets**: copy each reusable widget the user chose into `apps/<id>/`, and rewrite its manifest as "Shipping widgets" says.
   - **Check**: after a scan, the plugin detail page names any file the engine left out and the limit it broke.
3. **Find every external reference, then ask the user how to handle each one.** Walk the apps' HTML/JS/CSS for `src=`, `href=`, `import`, and `fetch(...)` calls. A path that does not resolve to a file you ship under the plugin tree needs a decision: absolute paths, and paths into `data/artifacts/`, another app's tree, or unshipped `data/scripts/` / `data/knowhow/`. **List each one back to the user and ask what to do** before bundling. Never silently rewrite or drop a reference. Per reference, the user picks one of:
   - (a) bundle the asset by copying it into `apps/<id>/` (or the right plugin subtree) and rewriting the reference;
   - (b) leave the reference as-is, because the installer provides the file separately (rare: document this in the plugin's README or `description`);
   - (c) delete the reference and the dependent feature;
   - (d) abort packaging.

   Ask because the engine cannot guess. An image in `data/artifacts/foo.png` might be a source of truth the user wants to share, or scratch they want to drop. Auto-bundling risks shipping private workspace artifacts, and auto-dropping risks publishing a broken feature.
4. **Bump `version` in `manifest.toml` before publishing.** Without a bump, `check_plugin_updates` reports `"Already at latest"` to existing installers, and they never get the new content. Adding `engine` for the first time needs its own bump too (see "The `engine` requirement" above).
5. **For archive distribution, package as zip and verify.** From inside the plugin tree, run `zip -r ../my-plugin.lucidos-plugin .`. Then check that `unzip -l ../my-plugin.lucidos-plugin` lists every expected file. `file ../my-plugin.lucidos-plugin` must report `Zip archive data`, not `gzip compressed data`.
6. **Commit and push.** For monorepo plugins, the install URL changes only if the subpath changes. `update_plugin` re-fetches new content under the same path.

Authors need no manual smoke test: the engine's e2e tests cover install, update and uninstall.

A second `install_plugin` over the same tree returns `Error: would overwrite N files: [list]. Re-run with overwrite=true to proceed.` The LLM relays that message verbatim. Running again with `overwrite=true` replaces each file atomically (write to `<dest>.tmp`, rename), so a crash mid-extract leaves no half-written file. The conflict scan runs before any write to `data/`, so a validation failure leaves the workspace untouched.

A disk failure during extract (out-of-space, permission denied) returns an error mid-write but does NOT roll back files already written. No install record is emitted, so a failed install can leave a partial set of files on disk.

## Versioning and updates

Semver is enforced at parse time: `0.1.0`, `1.4.2-beta.1` parse, `latest` and `1.0` do not.

`check_plugin_updates(id?)` (`engine/tools/plugins.rs::execute_check_plugin_updates`):

- With `id` omitted, surveys every currently-installed plugin (newest `PluginInstalled` event for each `aggregate_id`, skipped if a later `PluginUninstalled` exists).
- For each plugin, fetches the manifest from the recorded `manifest.source` (shallow clone to temp, read `manifest.toml`, discard).
- Compares semver. `changed: true` only when the remote version is strictly greater.
- Checks the remote manifest's `engine` requirement. It reports `engine_requirement` (as authored, or `null` when undeclared), `engine_compatible`, and, when it is false, `engine_incompatible_reason` with the same sentence `update_plugin` would refuse with. `changed: true` with `engine_compatible: false` means a newer version exists that this release cannot install yet.
- A network failure for one plugin becomes an `error` entry in the JSON output. It does not abort the whole check.

```json
[
  { "id": "browser-learning", "installed_version": "0.1.0", "latest_version": "0.2.0", "changed": true, "engine_requirement": null, "engine_compatible": true, "engine_incompatible_reason": null, "source": "...", "remote_manifest": { ... } },
  { "id": "theme-studio", "installed_version": "0.1.0", "latest_version": "0.2.0", "changed": true, "engine_requirement": ">=0.99.0", "engine_compatible": false, "engine_incompatible_reason": "Theme Studio 0.2.0 needs Lucidos 0.99.0 or later. This is Lucidos 0.46.2. Update Lucidos first.", "source": "...", "remote_manifest": { ... } },
  { "id": "habit-tracker", "installed_version": "1.4.0", "latest_version": "1.4.0", "changed": false, "engine_requirement": null, "engine_compatible": true, "engine_incompatible_reason": null, "source": "..." },
  { "id": "weather-feed", "installed_version": "0.3.0", "source": "...", "error": "fetch failed: ..." }
]
```

`update_plugin(id)` (`execute_update_plugin`):

- Looks up the newest `PluginInstalled` for `id` (must not be followed by `PluginUninstalled`).
- Re-fetches the remote manifest from the recorded `source`.
- If `remote_version <= installed_version`, returns `Already at latest (v<x>)`: a no-op that emits no event. `compare_versions` treats remote-older-than-installed as `AlreadyLatest`, so `update_plugin` cannot downgrade.
- Otherwise re-runs `install_plugin` with the recorded source and `overwrite=true`. It goes through the same staging, so an unmet `engine` requirement refuses the update just as it refuses an install. Same conflict mechanics, same `PluginInstalled` event variant: updates are installs over existing files.

If the recorded manifest has no `source`, the update returns an error rather than guessing.

A version that fails to parse (`compare_versions` with garbage on either side) counts as needing an update. The engine prefers attempting the install over silently doing nothing on corrupted data.

## Uninstall semantics

Uninstall stages a confirmation panel, just like install, and deletes nothing until the user confirms. The `uninstall_plugin(id)` tool (`prepare_uninstall_plugin`) and the Plugins panel's **Uninstall** button (`stage_uninstall_request`) open the same panel. Staging:

- Resolves `id` (or the manifest `name`, or an owned `apps/<dir>` folder) and reads the newest `PluginInstalled` record.
- Splits the recorded files into `files_present` and `files_missing`, and lists both in the panel. A recorded path outside the seven content dirs counts as missing, so the panel never offers it for deletion.
- Expires after 1 hour, like an install.

**Confirm** (`confirm_pending_uninstall`, then `uninstall_with_bus`):

1. Deletes each present file under `data/` and prunes empty parent folders. A content-dir root such as `data/apps/` always stays.
2. Commits the deletion in one commit, `"Uninstall plugin: <id> v<version>"`.
3. Emits `PluginUninstalled` with `files`, `files_deleted` and `files_missing`. A file that vanished between staging and confirm goes into `files_missing`.
4. Deletes the triggers stamped with this plugin's id, and reloads WASM signers if an `auth-modules/` file went.

The confirm response carries `files_deleted`, `files_missing` and a `summary` such as `Uninstalled Browser Learning v0.1.0 (2 files removed).` **Cancel** drops the staging and emits `PluginUninstallCanceled`.

What this means for plugin authors:

- **Uninstall deletes edited files too.** Confirming removes every recorded file still on disk, local edits included. The panel lists them first, and the workspace git history keeps each committed version. If your plugin invites edits, say so in its `description`.
- **Sharing a path between plugins is allowed but messy.** If two plugins both ship `knowhow/sites/linkedin.com/selectors.md`, whichever installs second wins (overwrite). Uninstalling either one deletes the file, even though the other plugin still relies on it. Namespace your files, e.g. `knowhow/<plugin-id>-<topic>.md` or a dedicated subdirectory.
- **Reinstall after uninstall is supported.** `install_plugin` on the same source after `uninstall_plugin` works: the engine treats the uninstall as a tombstone, and the next install is fresh.

## Local modifications (the "Modified" badge)

A plugin's shipped content lives under `data/` like any other artifact. So the user, the Lucidos Agent or a coding-agent thread can edit it after install. When that happens the Plugins list shows a **Modified** badge on the plugin's row. An update merges those changes into the new version where it can (see "An update keeps your changes" below).

This state is **derived on read, never stored**: there is no "PluginModified" event. The engine diffs the plugin's on-disk content against the install commit (`payload.data.manifest.commit`). It returns `modified` + `modified_paths` on the installed summary and catalog row (`registry::plugin_modification_status`). Being a pure function of git plus disk, it **self-heals**: revert an edit and the badge clears. An update re-stamps the baseline, so the badge clears there too, unless the update kept a patch. A kept patch is still a local change, so the badge correctly stays on.

What counts as a modification, per content type:

- **Apps** (`apps/<id>/`): any edit, delete, or **added** file inside the plugin's app directory (a directory diff against the install commit). Build output never counts: a file under one of the build-output directories listed in "Plugin layout", or a `*.pyc` / `*.pyo` file.
- **Knowhow / scripts / auth-modules**: an edit or delete of a file the plugin *recorded*. A brand-new file you drop into `knowhow/` (etc.) is *not* attributed to a plugin, since the user and other content share those roots.
- **Triggers** (`triggers/<slug>/trigger.toml`): a change to the trigger's *definition*. `trigger.toml` is a gitignored, re-serialized projection (ADR 0019). It is compared semantically (ignoring `slug` / `plugin_id` / `group_id`), not byte-for-byte, so re-serialization after install never counts.

### An update keeps your changes

An update three-way merges each locally-edited file. The three inputs are all in hand at staging time. **Base** is the file at the install commit, **ours** is what is on disk, and **theirs** is the staged new version (`plugins::merge`).

The staged install panel lists the outcome for each edited file **before** you confirm:

| Outcome | What the confirm does |
|---|---|
| `merged` | Writes the merged content. Your edit and upstream's both survive. |
| `conflict` | Both sides changed the same lines. Writes upstream's version, and saves yours aside. |
| `replaced` | Never mergeable: a trigger definition, a binary, or a file over 1 MB. Writes upstream's version, and saves yours aside. |
| `restored` | You had deleted the file and the new version still ships it, so it comes back. Nothing is saved aside, because a deletion has no content to keep. |

**A file already identical to the new version is not listed at all.** That is the common case after your own edit was accepted upstream. Those paths are written as shipped, with no row, no count, and nothing saved aside. The comparison is by content hash, so an oversized file is judged like any other.

The panel also carries one **Keep my edits** control, on by default. Clearing it takes a clean update: every file becomes `replaced`, and every edit is saved aside. The choice reaches the engine as `?keep_local_changes=false` on the confirm. An absent flag means keep, so a caller that never showed the control cannot silently discard a patch.

**A discarded edit is never simply deleted.** Before anything is overwritten, the engine writes your version and a `.patch` of it under `data/artifacts/plugin-local-changes/<plugin-id>/v<version>/`, with a `README.md` explaining the folder. That root is git-tracked and never auto-deleted. A clean merge saves nothing, because your edit survives in the file itself.

**The panel never opens blind.** Staging reads every installed plugin's install commit from the event log. When that read fails, the engine cannot tell which files you edited. So staging refuses any install that would overwrite an existing file, and tells you to try again. A fresh install has nothing to lose and stages as usual.

Installing one version twice is allowed, so a second save of the same version lands in `v<version>-2` rather than replacing the first. The engine never writes over a folder it already saved edits into.

**A conflict gets no conflict markers.** These files are LLM context: `knowhow/*.md` loads into the agent's prompt as instructions. A file containing `<<<<<<<` is a corrupted instruction the engine acts on. So upstream's coherent version wins on disk, and yours is preserved beside it.

**`trigger.toml` is never text-merged.** The engine rewrites it after every install, so its bytes never match what the plugin shipped, and every update would conflict. A changed trigger definition reports as `replaced`, and the shipped definition wins.

**The install commit stays a pristine copy of what the plugin shipped.** A merged path is recorded from upstream's bytes, not from the working tree (`core::commit_data_paths_with_overrides`). A second commit then records the merged tree plus any saved-aside copies. That lets a patch carry forward across updates. Otherwise the *next* update would read your patch as upstream's content, find no local modification, and drop it.

### Proposing your patch upstream

Whenever the Modified badge is up, the Plugins row offers **Propose upstream**. The install receipt offers it too, right after an update that kept a patch. `POST /api/v1/plugins/propose-upstream` with `{ "id": "<plugin-id>" }` does three things and stops:

1. Derives the diff from the install commit to the working tree, over the plugin's edited paths. Since the install commit is pristine, this is a patch against the version you are actually on.
2. Writes it to `data/artifacts/plugin-local-changes/<plugin-id>/proposed-v<version>.patch`, commits it, and emits `ArtifactCreated`.
3. Spawns a Lucidos Agent thread seeded with the plugin name and the patch path, returning its `thread_id` so the caller can navigate there. The seed is attributed to the engine with the reason `plugin_upstream_proposal`, beside the device that asked (`system-knowhow/thread-events.md` § Engine origins).

**The engine performs no GitHub operation.** No credentials, no fork, no API call. The spawned thread does that work, following the procedure below.

#### Procedure for the spawned thread

You have been asked to propose a user's local plugin changes to the plugin's author. Work in this order:

1. Read the patch named in the seed. It is a unified diff against the version the user has installed, with paths relative to the workspace repo root.
2. Find the plugin's upstream repo. `plugins(action="check_updates", id="<plugin-id>")` reports the recorded `source`, which is the manifest's git URL.
3. Read the change and judge whether it is upstreamable. A fix for a bug every user of the plugin hits is. Something specific to this user's setup, or carrying private data, is not: say so plainly and stop.
4. Ask the user to confirm before anything leaves the workspace, and agree the PR title and body with them.
5. Clone the source repo, apply the patch to the plugin's subdirectory, and open the pull request with `gh`. Report the URL back.

Never push to the upstream repo directly, and never open a PR without the user's confirmation in step 4.

## Events emitted

Three `SystemEvent` variants for the install lifecycle, plus the two cancel-audit ones. All carry `actor: Option<MessageOrigin>`, all have `aggregate()` returning `"plugin"`, and the `aggregate_id` is the plugin's `id` field.

### `PluginInstalled`

Emitted on every successful install, overwrites and updates included (there is no separate "PluginUpdated"). Persisted (`payload` JSONB) shape:

```json
{
  "type": "PluginInstalled",
  "data": {
    "id": "browser-learning",
    "manifest": {
      "summary": "Installed Browser Learning v0.1.0 from github.com/lucidos-dev/plugins/tree/main/browser-learning",
      "manifest": { "id": "browser-learning", "version": "0.1.0", "name": "...", "description": "...", "source": "https://github.com/lucidos-dev/plugins/tree/main/browser-learning", "engine": "...", "setup": "..." },
      "files": ["knowhow/browser-skills.md", "..."],
      "installed_at": "2026-04-29T12:34:56+00:00",
      "source_type": "git",
      "commit": "<workspace-repo sha of the install commit>",
      "setup_thread_id": "<uuid of the spawned setup thread, when one was spawned>"
    },
    "files": ["knowhow/browser-skills.md", "knowhow/browser-knowhow-reflection.md"],
    "installed_at": "2026-04-29T12:34:56+00:00",
    "source_type": "git",
    "actor": { ... }
  }
}
```

The two outer wrappers come from how Lucidos persists `SystemEvent`: serde's `tag = "type", content = "data"` adds `{type, data}`, and `install_from_unpacked_with_bus` packs the raw manifest into a payload map under `manifest` before assigning that map to `SystemEvent::PluginInstalled.manifest`. Net effect: the raw manifest fields (`version`, `source`, `setup`, ...) sit at `payload.data.manifest.manifest.*`. `InstalledRecord` reads them at that path; do the same in any new consumer. `setup_thread_id` sits one level up, at `payload.data.manifest.setup_thread_id`, because the engine records it rather than the author.

`id` is the exception, and deliberately so: it is at `payload.data.id`, where every sibling `Plugin*` frame carries it. It is still inside the manifest too. Rows written before the top-level field existed carry it only there, so a reader that must cover history falls back to `payload.data.manifest.manifest.id`.

**Two paths name the same value, and which one you write depends on who is reading.**

| You are… | Path to the plugin id | Why |
|---|---|---|
| reading an event with the `events` tool (`action=query`) | `payload.data.id` | You get the stored row, envelope and all. |
| writing a `condition` on a trigger or an `await_event`, or reading `TRIGGER_EVENT_PAYLOAD` in a script trigger | `id` | Both see the payload with the `type` / `data` envelope already stripped, so neither names those two keys. |

Same rule for anything deeper: `payload.data.manifest.manifest.version` when reading, `manifest.manifest.version` in a condition. Getting this wrong is silent at match time. So the engine checks every condition path against recent stored payloads when you subscribe. It names the real path when yours is in none of them. See `system-knowhow/triggers.md` § "What a condition can say".

`source_type` is `"git"` for both plain git URLs and GitHub tree URLs, `"archive"` for `.lucidos-plugin` installs.

`files` is the same list at the top level (`payload.data.files`) and inside the nested `manifest` blob (`payload.data.manifest.files`). `latest_install` reads the nested copy to find which files uninstall deletes.

`commit` (at `payload.data.manifest.commit`) is the workspace-repo sha of the "Install plugin: ..." commit. It is the baseline the Modified badge diffs against (see "Local modifications" above). Legacy rows installed before this field existed never show as modified.

Some old rows carry `aggregate_id = "unknown"`, from a since-fixed bug, and `latest_install(pool, &id)` cannot find them. Reinstall the plugin to refresh.

### `PluginLocalChangesMerged`

Emitted right after the `PluginInstalled` it belongs to, and only when the install met at least one locally-edited file:

```json
{
  "id": "email-triage",
  "version": "0.3.0",
  "merged": ["knowhow/email-triage.md"],
  "conflicted": ["apps/email-triage/index.html"],
  "replaced": ["triggers/email-triage/trigger.toml"],
  "saved_paths": ["artifacts/plugin-local-changes/email-triage/v0.3.0/apps/email-triage/index.html"],
  "commit": "<sha of the keep-local-changes commit>",
  "actor": { ... }
}
```

This one IS stored, unlike the Modified badge beside it, and the difference is the point. The badge is a pure function of git plus disk, so it can always be recomputed. A merge is not: once the two commits are written, nothing on disk says which files merged and which lost. `saved_paths` names the copies of every discarded edit, each with a `.patch` sibling.

### `PluginUninstalled`

```json
{
  "id": "browser-learning",
  "version": "0.1.0",
  "files": ["knowhow/browser-skills.md", "knowhow/browser-knowhow-reflection.md"],
  "files_deleted": ["knowhow/browser-skills.md"],
  "files_missing": ["knowhow/browser-knowhow-reflection.md"],
  "actor": { ... }
}
```

`files` is every recorded path, and `files_deleted` plus `files_missing` split it. Each list is omitted when empty. Rows from before uninstall deleted files carry neither.

Both events are useful trigger sources. Examples worth considering:

- A `PluginInstalled` trigger that runs the new plugin's smoke test or pins its app to the launcher.
- A `PluginUninstalled` trigger that offers to remove the plugin's runtime state under `data/artifacts/<plugin-id>/`, which uninstall leaves alone.

## Common mistakes to avoid

- **Building the archive with `tar`, `tar -czf`, or `gzip`.** A `.lucidos-plugin` is a renamed PKZip file. See "3. Local archive" and "Authoring loop" step 4.
- **Calling the manifest `manifest.json`, `manifest.yaml`, or anything other than `manifest.toml`.** `validate_tree` looks for `manifest.toml` at the archive root and only parses TOML. Other names or formats reject the archive before any file is written. See the schema table above for the fields.
- **Silently dropping or rewriting external references** (`<img src="../../artifacts/foo.png">`, `<script src="/data/scripts/bar.js">`). Ask the user about each one: see "Authoring loop" step 2.
- **Putting a README at the plugin root.** Validation rejects any top-level entry that is not `manifest.toml` or one of the seven content directories. Put your README inside `apps/<id>/` if it is app-specific, or only in the source repo, which install never reads.
- **Committing `__pycache__/` or `*.pyc` files.** Install skips them (see "Plugin layout"), but they still bloat the repo and add noise to every PR. Give the plugin repo a `.gitignore` that lists `__pycache__/`, `*.pyc` and any other build output your scripts or apps create.
- **Writing runtime state inside the plugin's own folders.** See "Where a plugin keeps its runtime state".
- **Using underscores or capitals in `id`.** `browser_learning` and `Browser-Learning` both fail validation. Stick to `[a-z0-9-]+`.
- **Setting `source` to a `.lucidos-plugin` path.** When present, `source` must be a git URL. For archive-only distribution, omit `source`.
- **Forgetting to bump `version` before publishing a fix.** Existing installers see `"Already at latest"` and never get your change.
- **Expecting an update to keep every user edit.** An update merges edits, but a conflict, a binary, a `trigger.toml` or a file over 1 MB takes the new shipped version. The user's copy is saved aside (see "An update keeps your changes"). If the user is meant to edit `knowhow/sites/linkedin.com/selectors.md`, ship it as a template they copy elsewhere.
- **Cross-plugin path collisions.** See "Sharing a path between plugins" under "Uninstall semantics".
- **Keeping user data in a shipped file.** Confirming an uninstall deletes every recorded file still on disk, edits included. Keep runtime state under `data/artifacts/<plugin-id>/`, which uninstall never touches.
