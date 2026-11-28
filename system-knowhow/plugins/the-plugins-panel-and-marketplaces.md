---
name: Plugins, The Plugins panel and marketplaces
description: Part of Plugins. Its index lists the sections it covers.
---

One part of `system-knowhow/plugins`, which lists every part.

## The Plugins panel and marketplaces

Users find and install plugins, and the *apps* they ship, in the **Plugins panel**. It is one list with an **Installed only** filter, checked by default:

- **Checked**, it lists every plugin on disk, *whatever it ships*. It reads the `PluginInstalled` projection via `GET /api/v1/plugins/installed`, so it works offline and still lists a plugin whose marketplace was removed.
- **Unchecked**, it widens to the full catalog: installed plus available from registered marketplaces.

The installed view is the home for plugins that ship **no app** (knowhow-, trigger-, script- or auth-module-only bundles). Each row shows the plugin's name, description and one button. An installed plugin with nothing to open says **Installed** instead. Its detail page holds the version, the **Modified** badge, the content kinds, file count and **Uninstall**.

When a registered marketplace offers a newer version, the row's button reads **Update**. The match is by plugin *id* (not `app_id`), so it works for app-less plugins too. Clicking stages the same confirmation panel as any install. A plugin's *app* still lives in the separate **Apps** panel, but the plugin itself, updates included, is managed here.

A **marketplace** is a git repository (or GitHub tree URL) registered at `data/config/plugin-marketplaces.json`, added and removed under **Settings → Marketplaces**. A scan clones each one, finds valid `manifest.toml` plugin roots, and compares each version against installed `PluginInstalled` events. The catalog shows each plugin as a card, read from the last scan (see "A scan never runs on a page open" below). `Install`/`Update` stage the same confirmation panel described above. The catalog never writes plugin files directly.

**Settings → Marketplaces** adds, renames and removes a marketplace. Only the name is editable there. The id hashes the URL, so a row's URL is read-only: pointing at another repository is a remove plus an add. The rename re-posts the stored URL, which is why the registry entry keeps its id.

Each card leads with the plugin icon: its *plugin media* icon, its single app's icon, or a *monogram tile*. It shows the manifest `description` cut to two lines. A tap on the card opens its **plugin detail page** in the content pane. The page holds:

- the full description;
- every screenshot and video in one strip that scrolls sideways, where a screenshot opens full size;
- `media/README.md`, rendered as markdown;
- a muted note naming each media file the engine left out, and why.

The page carries the card's button, **Uninstall**, and the chips the card leaves out. A modified plugin also gets a note counting the changed files, with **Send changes to author**. It shows media only: no plugin code runs there.

The README renders through the chat sanitizer. Lucidos drops its images, inline styles and form controls, since each would fetch from a host the author picked or draw over the page. List pictures as `screenshots` instead. Its web links open in a new tab, and a relative link shows as plain text.

Each card has one button, which progresses **Install → Setup → Open**. The detail page adds an **Uninstall** button once the plugin is on disk:

- **Install** (or **Update** for an out-of-date install): stages the confirmation panel. A plugin whose `engine` requirement this release misses still lists. Its button is disabled, and a warning chip gives the reason ("Needs Lucidos 0.46.1 or later"). The catalog row carries `engine_requirement` (as authored), `engine_compatible` and `engine_incompatible_reason`. They are filled each time the catalog is served, so an engine upgrade counts at once.
  - A plugin with no `engine` keeps a live button, and its detail page shows a muted "No version requirement" chip until it is installed. Its row has no `engine_requirement`.
  - An installed plugin whose marketplace is gone reads the same field from `GET /api/v1/plugins/installed`, which reports the installed version's requirement.
  - The two chips never show together, because an incompatible plugin always declares `engine`.
- **Setup**: shown after install while the setup thread is still running or waiting on the user. Clicking opens that thread. Driven by `setup_thread_id` + `setup_complete` on the catalog row. Plugins with no `setup` field skip past this. The engine resolves `setup_complete` from what it can observe about the thread:
  - **present** (has a `thread_summaries` row): done once its lifecycle status is neither `running` nor `waiting_for_user_answer`.
  - **pending** (no row yet but a live `thread_queue` entry, the brief window before the agent's first event): not done, so the card keeps showing Setup without flicker.
  - **gone** (no row and no queue entry: a lost spawn, deleted thread, or stale catalog id): treated as done. The card falls through to Open/Installed rather than offering a Setup button that would 404.
- **Open**: shown once setup is finished (or the plugin had none). It launches the plugin's primary app (`data/apps/<id>/`). Plugins that ship no app show the word **Installed** instead.
- **Uninstall** (detail page only): stages the uninstall confirmation panel (the same one the `plugins(action="uninstall")` LLM tool produces). The card re-fetches the catalog on every mount, so the Setup→Open transition shows when the user returns from setup.

**Plugin uninstall is the single removal authority for a plugin's app.** **Delete** in an app row's ⋯ menu on the Apps panel would `rm -rf` only `apps/<id>/`, leaving the plugin registered and its sibling `triggers/`/`knowhow/`/`scripts/` orphaned. So `DELETE /api/v1/app?id=...` returns **409** with `{ error, plugin_id, plugin_name }` when the app belongs to an installed plugin. That mirrors the `delete_file` guard, which refuses raw deletes of plugin-owned files. The UI catches the 409 and routes the user to the plugin **Uninstall** panel, which removes the whole plugin tree and emits `PluginUninstalled`. Standalone apps (no `PluginInstalled` record) keep deleting directly.

Installed marketplace plugins are **not** auto-updated: the engine notifies, and the user decides. It scans registered marketplaces at startup, after a marketplace is registered or renamed, and every five minutes. A newer version of an installed plugin produces one deduplicated `NotificationCreated` ("Plugin update(s) available") and installs nothing. Its tap opens the Plugins panel's installed list, with the **Installed only** filter on:

- **One update**: the list scrolls to and pulse-highlights that plugin, carried as the navigate `id`.
- **Several updates**: no row is highlighted, since each pending row's button already reads **Update**.

The user applies an update from either of two places, and each stages the same confirmation panel as any install:

- the plugin row's **Update** button, which works for *any* plugin, app or not;
- the catalog card, with **Installed only** unchecked.

**No notification for an update this release cannot install.** When the running release misses a version's `engine` requirement, nothing is sent. The Plugins panel still shows it, with Update disabled and the reason beside it. Once Lucidos meets the requirement, that version counts as new and the notification fires.

A `.lucidos/plugin-update-notice.json` marker tracks what the user was told. The re-scan re-notifies only for a *new* update (a fresh plugin or a bumped version), not every cycle.

**A scan never runs on a page open.** `GET /api/v1/plugins/catalog` reads the *plugin catalog cache* (`.lucidos/plugin-catalog.json`), which the five-minute scan writes, so the Plugins panel and Settings → Marketplaces paint at once. The response carries `scanned_at`, `scanning` and `scan_error` beside the rows. The panel shows no age, but names the reason above the list when the last scan failed. A cache older than five minutes, or none, starts a background scan, and the request still answers at once. `POST /api/v1/plugins/catalog/rescan` queues a scan now and returns at once.

The marketplace list itself is read live from the registry on every request. So a rename shows with no scan behind it, and a removed marketplace never contributes cached plugins. A scan announces both ends as **transient** events (SSE only, never stored): `PluginCatalogScanStarted` when it begins and `PluginCatalogScanned` when it lands, the latter carrying `failed`. They raise and lower the panel's scanning state on every connected client, the scheduler's own pass included.

Every marketplace mutation is **announced**, so an open Plugins panel and Settings → Marketplaces update in place with no reload. Registering a marketplace (or re-registering one under a new name) emits `PluginMarketplaceRegistered`. Unregistering emits `PluginMarketplaceRemoved`. The frontend re-reads the catalog on either. `Registered` is an upsert covering rename as well as create: the id hashes the canonical source, so re-registering a listed source rewrites its entry in place. The announcement lives in the one shared registry write path, so it fires for the HTTP endpoints below and the `plugins(action="register_marketplace")` tool alike.

Marketplace HTTP surface:

- `GET /api/v1/plugins/marketplaces` -> registered marketplace list.
- `POST /api/v1/plugins/marketplaces` with `{ "source": "...", "name"?: "..." }` -> register or rename a marketplace. Without a `name`, the default is the owner plus the repo, minus the word "lucidos" and any word the owner repeats: `acme/lucidos-plugins` becomes "acme plugins".
- `DELETE /api/v1/plugins/marketplaces/{id}` -> unregister a marketplace.
- `GET /api/v1/plugins/catalog` -> `{ marketplaces, plugins, errors, scanned_at, scanning, scan_error }`. `marketplaces` is live from the registry; `plugins` and `errors` come from the cache. Each installed plugin row also carries `setup_thread_id`, `setup_complete`, and `app_id` to drive the card's Install→Setup→Open button.
- `POST /api/v1/plugins/catalog/rescan` -> queue a marketplace scan. Answers `{ "queued": true }` at once; the result arrives as `PluginCatalogScanned`. A scan already running goes round once more after it, so the result comes from a scan that started after the request.
- `GET /api/v1/plugins/installed` -> `{ plugins }` from the `PluginInstalled` projection (no marketplace scan). Each row carries `id`, `name`, `version`, `source?` and `app_id?`. It also carries `content` (the shipped content-dir kinds), `files` (every installed `data/`-relative path), and `modified` + `modified_paths` (see "Local modifications" below). Backs the Plugins panel's installed-plugins view (the default **Installed only** filter) so it works offline and lists plugins whose marketplace was removed.
- `POST /api/v1/plugins/install-request` with `{ "source": "..." }` -> stage an install request payload for the existing confirmation panel.
- `POST /api/v1/plugins/uninstall-request` with `{ "id": "..." }` -> stage an uninstall request payload (resolves the plugin id, partitions its files into present/missing) for the uninstall confirmation panel. The button counterpart of the `plugins(action="uninstall")` LLM tool.
- `POST /api/v1/plugins/install/:install_id/confirm[?keep_local_changes=false]` -> write the staged files. The optional flag is the panel's keep control; absent means keep. The response adds `local_changes` (`merged` / `conflicted` / `replaced` / `saved_paths`) whenever the install met a locally-edited file.
- `POST /api/v1/plugins/propose-upstream` with `{ "id": "..." }` -> `{ patch_path, thread_id }`. Derives the plugin's local patch, writes it under `data/artifacts/`, and spawns a thread to take it to the author. See "Proposing your patch upstream" below.

Marketplace LLM surface:

- `plugins(action="register_marketplace", source, name?)` registers or renames a marketplace in the registry the Plugins panel browses. It commits `data/config/plugin-marketplaces.json` and announces the change, so any open panel refreshes. It then starts the scan / update-check pass, which notifies about plugin updates rather than applying them. Use it when a user asks to add a plugin repo, marketplace, or plugin marketplace source. There is no unregister tool: remove a marketplace from Settings → Marketplaces.

For GitHub monorepo marketplaces, register either the repo URL (`https://github.com/lucidos-dev/plugins`) or a tree URL (`https://github.com/lucidos-dev/plugins/tree/main/community`). The scanner turns discovered subdirectory plugins into installable GitHub tree URLs. For non-GitHub monorepos, use one repo per plugin: the install tool installs a subdirectory only from a GitHub tree URL.

