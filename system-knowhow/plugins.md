---
name: Plugins
description: Use when the user wants to author, package, publish, share, install, update, or uninstall a Lucidos plugin: "build a plugin", "publish a plugin", "share this app as a plugin", "package this for other workspaces", "ship a knowhow bundle", "install the X plugin", "update plugins", "make a .lucidos-plugin".
---

# Plugins

The working reference for *plugins*: packaging a coherent bundle of workspace content (apps + knowhow + triggers + scripts) so another workspace can install it as a unit, then distributing, updating, and uninstalling it. The full v1 design lives in `docs/plans/2026-04-29-plugins-v1-design.md`; this file is the operational reference. For setting up a plugin right after it is installed, see `plugin-setup.md`.

## When a plugin is the right artifact

A plugin is one bundle other workspaces install as a single unit. Use it only when the answer to "is this a coherent thing someone else would benefit from installing whole?" is yes.

| You want to share... | Right answer |
|---|---|
| One app the user pinned and likes | App, copy/paste the `apps/<id>/` tree -- no plugin needed for a single file or two |
| A standalone knowhow file | Knowhow file, paste it into `data/knowhow/` -- knowhow is already portable |
| An app + the knowhow it relies on + the trigger that drives it | Plugin -- the pieces only make sense together |
| A self-healing browser-skills loop (write-side knowhow + read-side reflection knowhow that a trigger calls) | Plugin -- canonical example, see `lucidos-dev/plugins/browser-learning` |
| A WASM auth signer (`<name>.wasm` + `<name>.manifest.json`) and the apis.json snippet that calls it | Plugin -- ship the signer in `auth-modules/` and use the `setup` field to walk the user through wiring `apis.json` + credentials at install time. See `system-knowhow/building-an-auth-handshake.md` for the signer ABI. |
| Engine defaults every workspace should always have | Not a plugin -- belongs in `system-knowhow/` |

The distinguishing test: removing any one file from the bundle would leave the others non-functional or actively misleading. If the files don't cohere, ship them separately.

## Questions to settle with the user before bundling

A plugin is a published artifact other workspaces install -- get the shape right before scaffolding. Skip questions the user has already answered.

1. **What's the cohesive unit?** List the files you intend to bundle and confirm each one would be misleading or non-functional without the others. If the answer is "they're related but each works alone", ship them as separate apps / knowhow / triggers, not a plugin.
2. **`id` and `name`?** `id` must match `[a-z0-9-]+`, max 64 chars -- confirm the slug. `name` is the human title.
3. **Distribution shape?** Single-repo git URL, monorepo subpath, or local `.lucidos-plugin` archive. Drives whether `source` is set in the manifest and where the user will publish.
4. **Cron triggers, OAuth, personal data?** None of those ship in plugins (see "What doesn't belong in a plugin" below). If the bundle would benefit from a cron trigger, surface that in the manifest `description` so the install-time LLM can offer to set one up -- confirm with the user that this is the intended UX.

## Plugin layout

A plugin is a directory containing `manifest.toml` at the root plus a subset of five content directories. The directories mirror `data/` one-to-one -- whatever lives at `<plugin>/<dir>/...` lands at `<workspace>/data/<dir>/...` at install time.

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
```

The `manifest.json` sidecar carries only WASM-host metadata (`secret_handles`, `body_mode`, `capabilities`). The engine never auto-loads provider config from it — `data/config/apis.json` is the single source of truth for proxy entries. Plugins that ship a signer should include the matching `apis.json` snippet in the manifest's `setup` field so the install-time LLM walks the user through pasting it into `data/config/apis.json` and registering the credential.

Validation rules enforced at install (`core/plugins.rs::validate_tree` and `validate_archive_entry_path`). Any failure rejects the archive before any file is written:

- `manifest.toml` exists at the plugin root and parses.
- All required manifest fields are present (`id`, `version`, `name`, `description`).
- `id` matches `[a-z0-9-]+`, non-empty, max 64 chars. Uppercase, underscore, dot all reject.
- `version` parses as semver.
- `source`, when present, looks like a git remote: starts with `https://`, `http://`, `git@`, or ends in `.git`. Bare strings, `file://` URLs, and `.lucidos-plugin` paths are not valid. `source` is optional -- omit it for archive-only plugins shared peer-to-peer (Slack drop, USB stick, attachment). `update_plugin` and `check_plugin_updates` will refuse a sourceless plugin with an explanatory error, but install and uninstall work fine.
- Top-level entries are exactly `manifest.toml` plus a subset of `{apps, knowhow, triggers, scripts, auth-modules}`. No root README, no `LICENSE`, no `.git`, no `node_modules`, no `__MACOSX` (auto-injected by macOS Finder when zipping). Put per-plugin docs and license inside the plugin's own subtree if needed.
- At least one of `apps/`, `knowhow/`, `triggers/`, `scripts/`, `auth-modules/` exists with at least one file. An empty `knowhow/` directory passes the top-level check but fails as `EmptyTree`.
- Hidden files (any path component starting with `.`) are silently skipped during the file walk -- `.DS_Store`, editor swap files, and friends do not get installed.
- No archive entry uses `..` or absolute paths (`/`, `\`) -- zip-slip protection.

The flat one-to-one mapping means there is no separate "install destination" question. If your file lives at `apps/foo/index.html` in the plugin, it lands at `data/apps/foo/index.html` in the workspace. Sub-trees (`triggers/foo/foo.md`, `apps/foo/sdk-prefs.js`) are preserved verbatim.

## `manifest.toml` schema

Four required fields, four optional. Unknown extra fields are accepted and round-trip into the `PluginInstalled` event payload's `manifest` so future additive fields stay compatible with old install records.

| Field | Required | Type | Notes |
|---|---|---|---|
| `id` | yes | string | `[a-z0-9-]+`, max 64 chars. Used as the install-record key, the event `aggregate_id`, and the canonical argument to `update_plugin` / `uninstall_plugin`. (`uninstall_plugin` also accepts the manifest `name` or any `apps/<dir>` folder name the plugin owns, case-insensitive — picks one if unambiguous, otherwise lists candidates.) |
| `version` | yes | string | Semver (`MAJOR.MINOR.PATCH`). `0.1.0`, `1.4.2-beta.1` both parse. |
| `name` | yes | string | Human-friendly title shown in install/uninstall messages. |
| `description` | yes | string | One-line summary. Free text. If your plugin pairs well with a cron trigger, mention it here (e.g. "Ask Lucidos to set up a daily reflection trigger after install") so the install-time LLM offers to wire one up -- see "What doesn't belong in a plugin" below. |
| `source` | no | string | Git remote URL where the plugin lives. Used by `check_plugin_updates` and `update_plugin` to re-fetch the manifest. Omit for archive-only sharing -- the plugin still installs and uninstalls correctly, but updates cannot be fetched (the update tools will return an explanatory error). When present, it must look like a git remote (`https://`, `http://`, `git@`, or ending in `.git`). |
| `engine` | no | string | Semver constraint (e.g. `">=0.5.0"`). Parsed and stored but the v1 install path does not enforce it -- use it as documentation for now. |
| `setup` | no | string | Markdown wiring instructions, written **to the agent about what to do with the user**. It renders in the install confirmation panel before the user confirms, and on confirm it drives a spawned Lucidos Agent setup thread. The engine never interprets it. This is the only way to ship workspace state rather than a file, a webhook above all. See "Install confirmation panel" below and `plugin-setup.md`. |
| `categories` | no | array of string | Topical tags for browsing the **Store** (the Plugins panel's category filter). A **controlled vocabulary** (see below) — pick from the allowed set. Normalised to lowercase on parse; an unknown value is **dropped and flagged** at catalog-scan time (it appears in the catalog's `errors`), never blocking install. |

**Plugin categories — the controlled vocabulary.** The allowed values are: `productivity`, `finance`, `health`, `developer-tools`, `data`, `communication`, `automation`, `lifestyle`, `research`, `fun` (kebab-case). The catalog offers a filter pill per category that actually appears in the catalog, and each card shows its category chips. The set is intentionally small and curated so categories stay browsable — a free-form tag would fragment (`finance` vs `money` vs `budgeting`). Tag a plugin with the one or few that fit; omit `categories` entirely if none do. (Source of truth: `PLUGIN_CATEGORIES` in `crates/lucidos-engine/src/core/plugins.rs`.)

Worked example (`browser-learning/manifest.toml`):

```toml
id = "browser-learning"
version = "0.1.0"
name = "Browser Learning"
description = "Self-healing site knowhow for browser automation. Agents emit observations during tasks; a reflection recipe folds them into per-domain knowhow so the next agent visits with better priors."
source = "https://github.com/lucidos-dev/plugins/tree/main/browser-learning"
categories = ["automation", "developer-tools"]
```

The `source` may be the GitHub tree URL the user copied from the address bar -- the install tool parses it back into a git remote + branch + subpath. For a single-repo plugin, use the bare git URL.

## Install confirmation panel

`install_plugin` (and `update_plugin`) never write directly to `data/`. The engine fetches + validates into a staged temp dir, then surfaces a confirmation panel in the Lucidos UI -- same content-pane surface as a credential request. The panel shows:

- The plugin name + version + description from the manifest
- The `source` (git URL or archive path) and `source_type` (git / archive)
- Every `data/`-relative path the install will write (overwrites called out separately in yellow)
- The `setup` field rendered as markdown, if present

The user clicks **Confirm** (writes files, **commits them to the workspace git repo in one commit** — `"Install plugin: <id> v<version>"` — emits `PluginInstalled`, auto-reloads WASM signers if any `auth-modules/` files were touched) or **Cancel** (drops staging, emits `PluginInstallCanceled`). Until they click, no bytes hit `data/`. The install commit means a plugin's files are version-controlled exactly like `write_file`/`edit_file` writes — with history, recoverable on a hard reset, and visible to git-based backups. Uninstall is symmetric: confirming it deletes the recorded files and commits the deletion (`"Uninstall plugin: <id> v<version>"`) before emitting `PluginUninstalled`.

**Confirming does not close the panel: it becomes a receipt.** The same panel turns into a read-only record of what the engine actually wrote or deleted, with a timestamp and no buttons. The header's back arrow is how you leave it. It keeps the nav-history row the pending confirmation already held (relabelled "Installed <name>" / "Uninstalled <name>"). So the user can walk Back to it, or reload, and still see what happened. Cancel closes as before, since nothing happened.

**Setup runs on confirm — but only when there's *new* setup to run.** If the manifest carried a non-empty `setup` field, confirming the install spawns a **Lucidos Agent setup thread** and the user is navigated straight into it, so the author's "ask the user / wire this up" steps actually happen instead of sitting inert as panel text. The thread's first message is a short line — `Set up the newly installed <name> plugin.` — **not** a wall of agent instructions: the "how to run a plugin setup" guidance lives in `system-knowhow/plugin-setup` (the agent loads it — the chat system prompt nudges it to), and the agent reads your `setup` text by referencing the durable `PluginInstalled` event (that knowhow names the exact nested payload path). So the user sees a clean thread while the agent still gets everything it needs. This fires for **both** the Plugins panel's Install button and the agent's `install_plugin` tool (they share the confirm endpoint). The spawned thread's id is recorded in the `PluginInstalled` event (`manifest.setup_thread_id`) so the plugin's card can resolve it later. On an **update**, the setup thread spawns only when the new version's `setup` text actually **differs** (after trimming) from the currently-installed version's — a version bump that left `setup` unchanged re-runs nothing and navigates nowhere, since re-doing identical setup on every update is noise. A fresh install (or an update whose `setup` is new/changed) always spawns. The engine's background marketplace **update check** only notifies — it never installs — so it never spawns a setup thread.

**An update's setup thread starts from the last run, not from scratch.** Its seed says so (`Set up <name> again: its setup instructions changed since version <prior>.`, titled `Update <name> setup`), and `system-knowhow/plugin-setup` § 1 acts on it. That section diffs your two `setup` texts, reads the `PluginSetupCompleted` record the previous run wrote, and verifies what is already wired before asking anything. So a reworded `setup` costs the user a couple of questions rather than the whole interview.

What this means for plugin authors:

- **Lead with `name` and `description`.** They render at the top of the panel. A vague description ("Self-healing site knowhow for browser automation. Agents emit observations during tasks…") gives the user enough to decide; a single word ("browser-skills") doesn't.
- **Use `setup` for wiring instructions the agent should run after install.** It renders as markdown in the panel so the user sees the steps before confirming, and on confirm a Lucidos Agent setup thread is spawned — the agent references this text (from the `PluginInstalled` event, guided by `system-knowhow/plugin-setup`), plans it as a todo list, and walks the user through the steps, asking for anything it needs (credentials, choices) and doing the wiring it can (e.g. pasting the `apis.json` snippet for a signer plugin). Write `setup` as instructions *to the agent about what to do with the user*, not as a static checklist.
  This is also the only way to ship anything that is workspace state rather than a file, a webhook above all.
- **Updates inherit the same panel.** `update_plugin` re-fetches the source and routes through the same staging path -- the user reviews the new version's file list (added/changed/removed -- well, "would overwrite" for changed) before any bytes are written.
- **Staged installs expire after 1 hour.** A panel left open longer is silently discarded; the user has to re-call `install_plugin`. Engine restarts also drop in-flight stagings (the staged temp dir is gone). Don't author flows that expect the panel to sit open for a full day.

## The Plugins panel and marketplaces

Plugins — and the *apps* they ship — are discovered and installed in the **Plugins panel**, the browser UI for plugin discovery. The panel is one unified list with an **Installed only** filter (checked by default): checked, it lists every plugin on disk *regardless of what it ships* (read from the `PluginInstalled` projection via `GET /api/v1/plugins/installed`, so it works offline and still lists a plugin whose marketplace was later removed); unchecked, it widens to the full catalog (installed + available from registered marketplaces). The installed-plugins view is the home for plugins that ship **no app** — knowhow-, trigger-, script-, or auth-module-only bundles — showing each plugin's content kinds + shipped files (each file links to a preview) and an **Uninstall** button. When a registered marketplace offers a newer version, the row also shows an **Update available** chip and an **Update** button — resolved by cross-referencing the catalog by plugin *id* (not `app_id`), so it works for app-less plugins too; clicking stages the same confirmation panel as any install. (A plugin's *app*, if it ships one, still lives in the separate **Apps** panel — which has its own **Update** shortcut on the app row — but the plugin itself is managed here.) A **marketplace** is a git repository (or GitHub tree URL) registered in the workspace at `data/config/plugin-marketplaces.json`, added/removed under **Settings → Marketplaces**. The catalog clones each registered marketplace on refresh (it re-scans whenever it is shown — the panel opens or **Installed only** is unchecked), scans for valid `manifest.toml` plugin roots, compares each manifest version against currently installed `PluginInstalled` events, and shows each plugin as a card. Clicking `Install`/`Update` stages the exact same install confirmation panel described above; the catalog never writes plugin files directly.

Each card has a primary button that progresses **Install → Setup → Open**, plus an **Uninstall** button once the plugin is on disk:

- **Install** (or **Update** for an out-of-date install) — stages the confirmation panel.
- **Setup** — shown after install while the plugin's setup thread is still running or waiting on the user; clicking opens that thread. Driven by `setup_thread_id` + `setup_complete` on the catalog row. `setup_complete` is resolved three ways from what the engine can observe about the setup thread: **present** (has a `thread_summaries` row) → done once its lifecycle status is neither `running` nor `waiting_for_user_answer`; **pending** (no row yet but a live `thread_queue` entry — the brief window after spawn before the agent's first event) → not done, so the card keeps showing Setup without flicker; **gone** (no row and no queue entry — a lost spawn, deleted thread, or a stale catalog id) → treated as done, so the card falls through to Open/Installed rather than offering a Setup button that would 404. Plugins with no `setup` field skip straight past this.
- **Open** — shown once setup is finished (or the plugin had none); launches the plugin's primary app (`data/apps/<id>/`). Plugins that ship no app show a disabled **Installed** instead.
- **Uninstall** — stages the uninstall confirmation panel (the same one the `uninstall_plugin` LLM tool produces). The card re-fetches the catalog on every mount, so the Setup→Open transition shows up when the user returns from finishing setup.

**Plugin uninstall is the single removal authority for a plugin's app.** A plugin-installed app cannot be removed by the **Delete** button on the Apps panel — that would `rm -rf` only the `apps/<id>/` dir and leave the plugin registered as installed with its sibling `triggers/`/`knowhow/`/`scripts/` orphaned. So `DELETE /api/v1/app?id=...` returns **409** with `{ error, plugin_id, plugin_name }` when the app belongs to an installed plugin (the app-level mirror of the `delete_file` guard, which already refuses raw deletes of plugin-owned files). The UI catches the 409 and routes the user to the plugin **Uninstall** panel instead — which removes the whole plugin tree and emits `PluginUninstalled`. Standalone apps (no `PluginInstalled` record) keep deleting directly.

Installed marketplace plugins are **not** auto-updated — the engine notifies, the user decides. Registered marketplaces are scanned at startup, after a marketplace is registered or renamed, and every five minutes. When a scanned marketplace has a newer version of an already-installed plugin, the engine emits a single deduplicated `NotificationCreated` ("Plugin update(s) available") whose tap deep-links to the Plugins panel's installed list (the **Installed only** filter on) and scrolls to / pulse-highlights the plugin with the pending update (carried as the navigate `id`; with several updates it focuses the alphabetically-first by name, the rest stay chipped); it does NOT install anything. The user applies the update from that row's **Update button** (works for *any* plugin, app or not), the catalog card (with **Installed only** unchecked), or — for a plugin that ships an app — the app row's **Update** button on the Apps panel; each stages the same confirmation panel as any install. Dedup is tracked in a `.lucidos/plugin-update-notice.json` marker so the five-minute re-scan only re-notifies when a *new* update appears (a fresh plugin or a bumped version), not every cycle.

Every marketplace mutation is **announced**, so an open Plugins panel and Settings → Marketplaces update in place with no reload: registering a marketplace (or re-registering one under a new name) emits `PluginMarketplaceRegistered`, unregistering emits `PluginMarketplaceRemoved`, and the frontend re-scans the catalog on either. `Registered` is an upsert event covering the rename as much as the create, because the marketplace id hashes the canonical source: re-registering a source already on the list rewrites its entry in place rather than adding a second one. The announcement lives inside the one shared registry write path, so it fires whichever surface asked (the HTTP endpoints below, or the `register_plugin_marketplace` tool when the user asks in chat).

Marketplace HTTP surface:

- `GET /api/v1/plugins/marketplaces` -> registered marketplace list.
- `POST /api/v1/plugins/marketplaces` with `{ "source": "...", "name"?: "..." }` -> register or rename a marketplace.
- `DELETE /api/v1/plugins/marketplaces/{id}` -> unregister a marketplace.
- `GET /api/v1/plugins/catalog` -> live scan result `{ marketplaces, plugins, errors }`. Each installed plugin row also carries `setup_thread_id`, `setup_complete`, and `app_id` to drive the card's Install→Setup→Open button.
- `GET /api/v1/plugins/installed` -> `{ plugins }` from the `PluginInstalled` projection (no marketplace scan). Each row carries `id`, `name`, `version`, `source?`, `app_id?`, `content` (the shipped content-dir kinds), `files` (every installed `data/`-relative path), and `modified` + `modified_paths` (see "Local modifications" below). Backs the Plugins panel's installed-plugins view (the default **Installed only** filter) so it works offline and lists plugins whose marketplace was removed.
- `POST /api/v1/plugins/install-request` with `{ "source": "..." }` -> stage an install request payload for the existing confirmation panel.
- `POST /api/v1/plugins/uninstall-request` with `{ "id": "..." }` -> stage an uninstall request payload (resolves the plugin id, partitions its files into present/missing) for the uninstall confirmation panel. The button counterpart of the `uninstall_plugin` LLM tool.
- `POST /api/v1/plugins/install/:install_id/confirm[?keep_local_changes=false]` -> write the staged files. The optional flag is the panel's keep control; absent means keep. The response adds `local_changes` (`merged` / `conflicted` / `replaced` / `saved_paths`) whenever the install met a locally-edited file.
- `POST /api/v1/plugins/propose-upstream` with `{ "id": "..." }` -> `{ patch_path, thread_id }`. Derives the plugin's local patch, writes it under `data/artifacts/`, and spawns a thread to take it to the author. See "Proposing your patch upstream" below.

Marketplace LLM surface:

- `register_plugin_marketplace(source, name?)` registers or renames the same plugin marketplace registry the Plugins panel browses, commits `data/config/plugin-marketplaces.json`, announces the change (so any open panel refreshes without a reload), and kicks off the marketplace scan / update-check pass (which notifies the user of any available plugin updates rather than applying them). Use it when a user asks conversationally to add a plugin repo, marketplace, or plugin marketplace source. There is no unregister tool: removing a marketplace is done from Settings → Marketplaces.

For GitHub monorepo marketplaces, register either the repo URL (`https://github.com/lucidos-dev/plugins`) or a tree URL (`https://github.com/lucidos-dev/plugins/tree/main/community`). The scanner turns discovered subdirectory plugins into installable GitHub tree URLs. For non-GitHub monorepos, use one repo per plugin or provide a GitHub tree URL equivalent; the install tool only knows how to install a subdirectory when it has a GitHub tree URL.

## Authoring a marketplace

A marketplace is not its own artifact type -- there is no marketplace manifest,
no schema, no validation step. It is **a git repository whose subdirectories
contain plugin roots**. Everything above about authoring a plugin still applies
verbatim; this section only covers the repo that holds them.

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
- **At depth 0, the five content-dir names (`apps`, `knowhow`, `triggers`,
  `scripts`, `auth-modules`) are skipped** -- that guard stops a single-plugin
  repo (manifest at the root) from also reporting its own `apps/` as a
  candidate.
- **Duplicate plugin `id`s are de-duplicated** -- first root wins, later ones are
  silently dropped. Keep directory name == manifest `id` to make collisions
  obvious.
- A repo where the scan finds nothing fails with `no plugin manifest.toml files
  found`.

**Root-level files that are not plugin directories are ignored.** A README,
`CODEOWNERS`, `.github/`, `LICENSE`, `.gitignore` at the *marketplace* root are
fine -- the strict "only `manifest.toml` + the five content dirs" validation
applies **inside a plugin root**, not to the marketplace repo. Use the root
README as the human discovery index (this is what `lucidos-dev/plugins` does).

### Install URLs are generated, not authored

For a GitHub marketplace the scanner rewrites each discovered plugin root into a
tree URL (`install_source`): `https://github.com/<owner>/<repo>/tree/<branch>/<plugin-dir>`,
with the marketplace's own subpath prefixed when it was registered as a tree URL.
The branch is the one registered, else the cloned repo's actual HEAD shorthand.

Consequences worth designing around:

- **Subdirectory install only works for GitHub.** For a non-GitHub host, the
  fallback is the marketplace's own clone URL -- which is only correct if the
  repo *is* a single plugin. Multi-plugin marketplaces on GitLab / Bitbucket /
  an enterprise host do not produce installable per-plugin URLs in v1. Use one
  repo per plugin there.
- **Renaming a plugin directory changes its install URL** and orphans the
  `source` recorded in existing installs. Treat the directory name as stable.
- Each plugin's own `manifest.toml` `source` should still be set to its tree URL
  -- that is what `check_plugin_updates` / `update_plugin` re-fetch after
  install, independent of the marketplace.

### Registering and the scan cycle

Register the repo URL (`https://github.com/owner/repo`) or a tree URL to scope
the scan to a subdirectory (`.../tree/main/community`). `.lucidos-plugin`
archive paths are rejected -- marketplaces must be git. The clone is shallow
(`depth 1`), lands in `.lucidos/tmp/plugin-marketplaces/`, and `.git` is removed
before the scan, so nothing about the marketplace persists in the workspace
except the registry entry in `data/config/plugin-marketplaces.json`.

A `file://` URL is a valid marketplace source, and it clones deep rather than
shallow: libgit2's local transport rejects a shallow fetch. A local bare clone
of a repo, refreshed out of band, is a working offline marketplace.

Scans run at startup, on registration/rename, whenever the Plugins panel is
shown or "Installed only" is unchecked, and every five minutes. A scan **never
installs** -- it notifies about newer versions and the user clicks Update.

### Private and internal repos

**The clone authenticates.** One shared helper, `core/git_auth.rs`, supplies
credentials for every clone the engine makes: the marketplace scan
(`clone_marketplace`), the plugin install and update fetch
(`tools/plugins/source.rs`), and both `git_clone` tool routes. A public repo is
unaffected. libgit2 asks for a credential only when the remote demands one.

**A token comes from Settings, Credentials, and nowhere else.** No environment
variable holds one. Before the clone starts, the engine looks up the credential
whose **Base URL** scopes the clone URL, and hands that one credential to the
clone.

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
| Base URL | `https://github.com` for github.com. For a GitHub Enterprise install, its own host, `https://github.example.io`. |
| Auth Type | **Bearer Token**, with a token that can read the repo. |

**Base URL is the whole scoping rule.** It must be the host you clone from, so
`https://github.com`, **not** `https://api.github.com`. Those are different
hosts, and a credential registered for the API host never matches a clone. A
credential for the REST API and a credential for the clone are two rows.

A Base URL may also carry a path, `https://github.com/example-org`, which scopes
it to that owner. When several credentials match one URL the longest Base URL
wins, so an org-scoped token overrides a host-wide one.

A GitHub Enterprise install needs nothing extra beyond its own row. The clone
never guesses which host is GitHub, so there is no host list to maintain.

The row takes effect on the next scan, with no restart: every clone reads the
store when it starts. Scans run every five minutes and whenever the Plugins
panel opens.

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

Creating the marketplace repo is a GitHub-side concern and fails separately from
anything Lucidos does. An org with `members_can_create_repositories: false`
rejects `gh repo create` for a plain member with
`does not have the correct permissions to execute CreateRepository` -- for
public, private, **and** internal alike. Check with
`gh api orgs/<org> --jq '{members_can_create_repositories, members_can_create_internal_repositories}'`
before assuming the CLI or the token is at fault.

## Shipping triggers (auto-registration)

A plugin ships a trigger by declaring it in a **`trigger.toml`** at
`triggers/<slug>/trigger.toml` — mirroring how an app is its own folder
(`apps/<id>/manifest.json`). The file is a *trigger definition* (see
`triggers.md` § "On-disk trigger definition"): `name`, `run`
(`intent` or `script`), `on` (trigger subscriptions), and the usual optional
fields (`app_id`, `go_to_review`, `group_id`, `side_effect_grant`, and
`model` / `reasoning_effort` to pin the intent to a specific chat model). Put any
procedure the trigger needs in `triggers/<slug>/knowhow/`, beside it.

What install does (ADR 0019):

- **Auto-registers** each `trigger.toml` — emits `TriggerCreated` stamped with
  the plugin's id (provenance), so the trigger is **live immediately** (no agent
  step needed). The Triggers panel shows a "from \<plugin\>" chip on it.
- **Event-driven only.** A `trigger.toml` that declares a cron `schedule` is
  **rejected at install** (nothing is written) — cron is workspace state, not
  plugin content. Ship `on:` subscriptions; if the plugin pairs well with a cron
  cadence, say so in the manifest `description` so the install-time LLM offers to
  set one up conversationally.
- **Uninstall** auto-deletes exactly the triggers carrying this plugin's id
  (user-created triggers are never touched).
- **Update** re-syncs by `(plugin_id, slug)`: a still-declared slug is updated in
  place (preserving the user's paused state), a new slug is created, a dropped
  slug is removed.

The user sees the `trigger.toml` files in the install confirmation panel's file
list (with their `side_effect_grant` visible in the parsed definition) before
confirming, so activation is never silent.

## What doesn't belong in a plugin

The four content directories make almost anything technically packageable, but apply judgment to `triggers/`. Apps, knowhow, and **event-driven (`on_event`) triggers** belong in plugins -- they are reference material or part of the plugin's own mechanism. An `on_event` trigger that reacts to events the plugin's apps/knowhow emit ships as a `triggers/<slug>/trigger.toml` declaration: install **auto-registers** it (stamped with the plugin id) and uninstall removes it — see "Shipping triggers" above.

**Cron triggers, OAuth credentials, and personal data do not ship in plugins.** They are workspace state -- WHEN something runs on a clock, WHO owns the account, WHAT the user has accumulated. Cron triggers in particular get singled out -- four reasons:

1. **Cadence is user-specific.** Heavy users want it every 6h, light users weekly. Hardcoding `0 0 4 * * *` in the bundle makes that decision for them.
2. **The schedule is workspace state, not reference material.** A plugin shipping a cron entry is the equivalent of a library shipping a crontab line -- wrong layer. Knowhow is "how to do this well", cron triggers are "when I want it to happen".
3. **Orphaned cron entries.** If the install instructions create a cron trigger as a side effect (asking an agent to call `create_trigger`), it carries no plugin provenance, so uninstall does not know to remove it — the workspace ends up with an orphaned cron entry pointing at deleted knowhow. (Event-driven triggers declared as `triggers/<slug>/trigger.toml` avoid this: install auto-registers them stamped with the plugin id, and uninstall auto-deletes exactly those — see "Shipping triggers".)
4. **Install-time prompt is the right UX.** When `install_plugin` lands the knowhow, the LLM tells the user *"This plugin works best with a reflection trigger. Want me to set one up? Daily at 4am is a good default."* Conversational, opinionated default, but the user owns the schedule.

**Webhooks do not ship either, and for the same reason.** A webhook is a row in the `webhooks` table, not a file, so there is no manifest field for one and no sixth content dir. Three things would have to travel with it and none can. The shared secret is a `credentials` row, and plugins never ship credentials. The delivery URL does not exist until create time, and the host is the installing machine's own funnel. The sender-side registration only the account owner can do.

Everything downstream of the event still ships, and it is most of the value: the `triggers/<slug>/trigger.toml` subscribing to the pinned event, the script that reads the payload, and the knowhow describing the payload shape and its field paths. The `setup` field is the bridge for the hook itself. Write it as instructions to the agent. Request the shared secret as a credential. Run `lucidos webhooks create` with the same event type the trigger subscribes to. Read back the delivery path, and hand the user the URL plus the exact steps to register it with the sender.

**A plugin doing this should say so in its `description`.** The trigger goes live at install, subscribed to an event type nothing emits until setup finishes. If the user cancels the setup thread, or the create fails, the trigger sits there never firing and nothing warns anyone. The same failure mode as a half-done event rename. The `setup` text should verify the hook exists before it reports done.

Concretely:

- Apps, knowhow, and event-driven (`on_event`) triggers ship in plugins -- they're reference material or part of the plugin's mechanism (what it IS, how to do it, what it reacts to).
- Cron triggers, OAuth credentials, and personal data DO NOT ship in plugins -- they're workspace state (WHEN-on-a-clock, WHO, WHAT-FOR).
- If a plugin would benefit from a cron trigger, the manifest `description` should mention it so the install-time LLM can offer to set one up conversationally.

The canonical example is `browser-learning` v0.2.0, which ships knowhow only and relies on the install-time prompt for the reflection (cron) trigger.

## Three distribution shapes

`install_plugin(source)` detects the shape by string format (`engine/tools/plugins.rs::detect_source`).

### 1. Single-repo plugin -- plain git URL

The plugin tree sits at the repo root.

```
github.com/owner/my-plugin
  manifest.toml
  knowhow/
  ...
```

Install URL: `https://github.com/owner/my-plugin` or `https://github.com/owner/my-plugin.git`. The engine shallow-clones the default branch.

Pick this when the plugin is a standalone project with its own README, issue tracker, and release cadence.

### 2. Monorepo with subpath -- GitHub tree URL

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

Install URL: `https://github.com/lucidos-dev/plugins/tree/main/browser-learning`. This is exactly what GitHub puts in the address bar when a user navigates to the plugin's directory in the web UI -- copy + paste install.

Parse rules (`parse_github_tree`): the URL must be `https://github.com/<owner>/<repo>/tree/<branch>[/<subpath>]`. The engine clones `https://github.com/<owner>/<repo>.git` at `<branch>`, then treats `<subpath>` as the plugin root. Subpath is optional (a tree URL pointing at the repo root works too).

Pick this when shipping multiple plugins together makes sense -- shared review cadence, one CI, one README listing them all. The canonical example is `lucidos-dev/plugins`. The repo's top-level README acts as a human-browseable discovery index; the engine ignores it (subpath isolates the plugin tree).

For non-GitHub monorepos in v1, fall back to one repo per plugin -- only GitHub tree URLs are parsed.

### 3. Local archive -- `.lucidos-plugin` file

A `.lucidos-plugin` is a **PKZip archive** of the plugin tree, renamed. Always build with `zip`:

```
cd my-plugin
zip -r ../my-plugin.lucidos-plugin .
```

**Do not use `tar`, `tar -czf`, `gzip`, or any non-zip format.** The custom `.lucidos-plugin` extension does not change the format -- the engine opens it with `zip::ZipArchive::new()` (`engine/tools/plugins.rs::extract_zip`), which only understands PKZip. A gzipped tarball or raw gzip stream fails with an opaque "read archive: ..." parse error and the user has to repackage. If `zip` is not installed, install it (`brew install zip`, `apt install zip`) rather than substituting another archiver.

Install URL: an absolute filesystem path ending in `.lucidos-plugin` (`/Users/x/Downloads/my-plugin.lucidos-plugin`). The engine extracts the zip into a temp dir and validates as if it were a git checkout.

Pick this for: ad-hoc sharing (Slack, email), pre-publication testing, plugins that cannot or should not be published to a public git host. The custom extension makes the file self-announcing and gives a clean upgrade path to OS file association later. Archive plugins may omit the `source` field entirely -- they install and uninstall normally, but `check_plugin_updates` / `update_plugin` will report that there is nowhere to fetch from. If you do want updates while still distributing as an archive, set `source` to the git repo the archive is built from.

## Authoring loop

1. **Lay out the tree.** Create `my-plugin/manifest.toml` and the content directories. Author content as if it were already installed -- knowhow files use the same frontmatter rules as any other knowhow (`system-knowhow/building-knowhow.md`), apps follow the app conventions (`system-knowhow/building-an-app.md`), triggers obey the intent-vs-procedure rule (`system-knowhow/triggers.md`).
2. **Find every external reference, then ask the user how to handle each one.** Walk the apps' HTML/JS/CSS for `src=`, `href=`, `import`, and `fetch(...)` calls. For each path that does not resolve to a file you're already shipping under the plugin tree -- absolute paths, paths into `data/artifacts/`, paths into another app's tree, paths into the workspace's `data/scripts/` or `data/knowhow/` you don't intend to ship -- **list it back to the user and ask what to do** before bundling. Do not silently rewrite or drop references. Per reference, the user picks one of: (a) bundle the asset by copying it into `apps/<id>/` (or the appropriate plugin subtree) and rewriting the reference, (b) leave the reference as-is because the installer is expected to provide the file separately (rare -- document this in the plugin's README or `description`), (c) delete the reference and the dependent feature, or (d) abort packaging. The reason for asking: an image in `data/artifacts/foo.png` might be the user's source-of-truth they want to share, or it might be incidental scratch they want to drop -- the engine cannot guess.
3. **Bump `version` in `manifest.toml` before publishing.** Without a version bump, `check_plugin_updates` will report `"Already at latest"` to existing installers and they will not pick up the new content.
4. **For archive distribution, package as zip and verify.** From inside the plugin tree, run `zip -r ../my-plugin.lucidos-plugin .`. Then verify with `unzip -l ../my-plugin.lucidos-plugin` that every expected file is present and `file ../my-plugin.lucidos-plugin` reports `Zip archive data` (not `gzip compressed data`). Never substitute `tar`, `tar -czf`, or `gzip` -- the install path only understands PKZip.
5. **Commit and push.** For monorepo plugins, the install URL changes only if the subpath changes -- bumping the plugin's content under the same path is what `update_plugin` re-fetches.

The engine's e2e tests cover install, update, and uninstall mechanics -- plugin authors don't need a manual smoke-test loop. Write a valid manifest and tree; the engine guarantees the rest.

A second `install_plugin` over the same tree returns `Error: would overwrite N files: [list]. Re-run with overwrite=true to proceed.` That message is verbatim what the LLM relays to the user; running again with `overwrite=true` proceeds and atomically replaces each file (write to `<dest>.tmp`, rename) so a crash mid-extract does not leave half-written content. The conflict scan happens before any write to `data/`, so the "no conflicts -> no overwrite needed" path leaves the workspace untouched on validation failure.

A disk failure during extract (out-of-space, permission denied) returns an error mid-write but does NOT roll back already-written files. This is rare in practice and the install record is not emitted -- but be aware that a failed install can leave a partial subset of files on disk.

## Versioning and updates

Semver is enforced at parse time -- `0.1.0`, `1.4.2-beta.1` parse, `latest` and `1.0` do not.

`check_plugin_updates(id?)` (`engine/tools/plugins.rs::execute_check_plugin_updates`):

- With `id` omitted, surveys every currently-installed plugin (newest `PluginInstalled` event for each `aggregate_id`, skipped if a later `PluginUninstalled` exists).
- For each plugin, fetches the manifest from the recorded `manifest.source` (shallow clone to temp, read `manifest.toml`, discard).
- Compares semver. `changed: true` only when the remote version is strictly greater.
- Network failures per plugin become `error` entries in the JSON output -- they do not abort the whole check.

```json
[
  { "id": "browser-learning", "installed_version": "0.1.0", "latest_version": "0.2.0", "changed": true, "source": "...", "remote_manifest": { ... } },
  { "id": "habit-tracker", "installed_version": "1.4.0", "latest_version": "1.4.0", "changed": false, "source": "..." },
  { "id": "weather-feed", "installed_version": "0.3.0", "source": "...", "error": "fetch failed: ..." }
]
```

`update_plugin(id)` (`execute_update_plugin`):

- Looks up the newest `PluginInstalled` for `id` (must not be followed by `PluginUninstalled`).
- Re-fetches the remote manifest from the recorded `source`.
- If `remote_version <= installed_version` returns `Already at latest (v<x>)` -- a no-op that emits no event. (Note: the `compare_versions` helper treats remote-older-than-installed as `AlreadyLatest`, so a downgrade also no-ops; intentional version downgrades are not supported by `update_plugin`.)
- Otherwise re-runs `install_plugin` with the recorded source and `overwrite=true`. Same conflict mechanics, same `PluginInstalled` event variant -- updates are just installs over existing files.

If the recorded manifest is missing `source` (which would only happen if a future plugin format changes the field), the update returns an error rather than guessing.

A version that fails to parse (`compare_versions` with garbage on either side) is treated as needing update -- the engine prefers attempting the install over silently no-oping on corrupted data.

## Uninstall semantics (v1 is GUIDE-ONLY)

`uninstall_plugin(id)` (`execute_uninstall_plugin`):

- Looks up the newest `PluginInstalled` for `id`.
- Emits a `PluginUninstalled` event with `{id, version, files}` (files copied from the install record).
- Returns text the LLM relays to the user listing every path under `data/` to delete:

```
Plugin "browser-learning" v0.1.0 marked uninstalled.

To remove its files, delete these N paths under data/:
  - knowhow/browser-skills.md
  - knowhow/browser-knowhow-reflection.md

Some files may have been edited since install, or shared with another plugin -- review before deletion.
```

The engine does NOT delete files. The LLM should offer "want me to delete them?" and chain to the existing file-delete tools once the user confirms.

What this means for plugin authors:

- **Design files to be tolerant of being installed alongside user edits.** A user who customised your knowhow file in place loses those edits if the LLM blindly deletes during the uninstall guide flow -- your README should warn about this if your plugin invites edits.
- **Sharing a path between plugins is allowed but messy.** If two plugins both ship `knowhow/sites/linkedin.com/selectors.md`, whichever installs second wins (overwrite). Uninstalling either one suggests deleting the file even though the other plugin still relies on it. Avoid path collisions across plugins where possible.
- **Reinstall after uninstall is supported.** Calling `install_plugin` on the same source after `uninstall_plugin` works -- the engine treats the uninstall as a tombstone and the next install is fresh.

## Local modifications (the "Modified" badge)

A plugin's shipped content lives under `data/` like any other artifact, so the user (or the Lucidos Agent, or a coding-agent thread) can edit it after install. When that happens the Plugins list shows a **Modified** badge on the plugin's row, and updating the plugin warns that the update will overwrite the local changes.

This state is **derived on read, never stored**: there is no "PluginModified" event. The engine diffs the plugin's on-disk content against the install commit (`payload.data.manifest.commit`). It returns `modified` + `modified_paths` on the installed summary and catalog row (`registry::plugin_modification_status`). Being a pure function of git plus disk, it **self-heals**: revert an edit and the badge clears. An update re-stamps the baseline, so the badge clears there too, unless the update kept a patch. A kept patch is still a local change, so the badge correctly stays on.

What counts as a modification, per content type:

- **Apps** (`apps/<id>/`): any edit, delete, or **added** file inside the plugin's app directory (a directory diff against the install commit).
- **Knowhow / scripts / auth-modules**: an edit or delete of a file the plugin *recorded*. A brand-new file you drop into `knowhow/` (etc.) is *not* attributed to a plugin — those roots are shared by the user and other content.
- **Triggers** (`triggers/<slug>/trigger.toml`): a change to the trigger's *definition*. `trigger.toml` is a gitignored, re-serialized projection (ADR 0019), so it is compared semantically (ignoring `slug` / `plugin_id` / `group_id`), not byte-for-byte — re-serialization after install never counts as a modification.

### An update keeps your changes

An update used to overwrite every file the plugin ships, so a local edit was lost. It is three-way merged instead. The three inputs are all in hand at staging time. **Base** is the file at the install commit, **ours** is what is on disk, and **theirs** is the staged new version (`plugins::merge`).

The staged install panel lists the outcome for each edited file **before** you confirm:

| Outcome | What the confirm does |
|---|---|
| `merged` | Writes the merged content. Your edit and upstream's both survive. |
| `conflict` | Both sides changed the same lines. Writes upstream's version, and saves yours aside. |
| `replaced` | Never mergeable: a trigger definition, a binary, or a file over 1 MB. Writes upstream's version, and saves yours aside. |
| `restored` | You had deleted the file and the new version still ships it, so it comes back. Nothing is saved aside, because a deletion has no content to keep. |

**A file already identical to the new version is not listed at all.** It is the common shape after you publish your own edit upstream: you update back onto your own work. Those paths are written as shipped, with no row, no count, and nothing saved aside. Only the files that still differ from the new version are a decision. The comparison is by content hash, so an oversized file is judged like any other.

The panel also carries one **Keep my local changes** control, on by default. Clearing it takes a clean update: every file becomes `replaced`, and every edit is saved aside. The choice reaches the engine as `?keep_local_changes=false` on the confirm. An absent flag means keep, so a caller that never showed the control cannot silently discard a patch.

**A discarded edit is never simply deleted.** Before anything is overwritten, the engine writes your version and a `.patch` of it under `data/artifacts/plugin-local-changes/<plugin-id>/v<version>/`, with a `README.md` explaining the folder. That root is git-tracked and never auto-deleted. A clean merge saves nothing, because your edit survives in the file itself.

Installing one version twice is allowed, so a second save of the same version lands in `v<version>-2` rather than replacing the first. The engine never writes over a folder it already saved edits into.

**Why a conflict does not get conflict markers.** These files are LLM context: `knowhow/*.md` is loaded into the agent's prompt as instructions. A file containing `<<<<<<<` is not a broken file you notice, it is a corrupted instruction the engine acts on. So upstream's coherent version wins on disk, and yours is preserved beside it.

**`trigger.toml` is never text-merged.** It is a re-serialized projection the engine rewrites after every install, so its bytes never match what the plugin shipped. Merging it would report a conflict for every plugin trigger on every update. A changed trigger definition reports as `replaced`, and the shipped definition wins.

**The install commit stays a pristine copy of what the plugin shipped.** A merged path is recorded from upstream's bytes, not from the working tree (`core::commit_data_paths_with_overrides`). A second commit then records the merged tree plus any saved-aside copies. That is what lets a patch carry forward indefinitely. Record the merge in the install commit instead, and the *next* update reads your patch as upstream's own content. It then finds no local modification, and drops it.

### Proposing your patch upstream

Whenever the Modified badge is up, the Plugins row offers **Propose upstream**. The install receipt offers it too, right after an update that kept a patch. `POST /api/v1/plugins/propose-upstream` with `{ "id": "<plugin-id>" }` does three things and stops:

1. Derives the diff from the install commit to the working tree, over the plugin's edited paths. Since the install commit is pristine, this is a patch against the version you are actually on.
2. Writes it to `data/artifacts/plugin-local-changes/<plugin-id>/proposed-v<version>.patch`, commits it, and emits `ArtifactCreated`.
3. Spawns a Lucidos Agent thread seeded with the plugin name and the patch path, returning its `thread_id` so the caller can navigate there.

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

Emitted on every successful install (including overwrites and updates -- the variant is reused, no separate "PluginUpdated"). Persisted (`payload` JSONB) shape:

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

`source_type` is `"git"` for both plain git URLs and GitHub tree URLs, `"archive"` for `.lucidos-plugin` installs. `files` is the same list at the top level (`payload.data.files`) and inside the nested `manifest` blob (`payload.data.manifest.files`) -- the nested copy is what `latest_install` reads when looking up "what files belong to this plugin?" for the uninstall guide. `commit` (at `payload.data.manifest.commit`) is the workspace-repo sha of the "Install plugin: ..." commit -- the baseline the Modified badge diffs against (see "Local modifications" below). Legacy rows installed before this field was recorded simply never show as modified.

> **Historical bug, fixed.** Earlier `InstalledRecord::source()` read `payload.manifest.source` -- two layers too shallow -- and silently returned `None`, surfacing as the misleading `"installed manifest is missing 'source' -- cannot fetch latest"` error from `check_plugin_updates` even when the source was recorded. The matching `aggregate_id()` derivation read `manifest.id` (also too shallow), which made every PluginInstalled row land with `aggregate_id = "unknown"` and broke `latest_install(pool, &id)` lookups. Both are fixed; the e2e tests in `engine/tools/plugins.rs` (the `e2e_*` cases) lock the round-trip in. Plugins installed before the fix may still have `aggregate_id = "unknown"` in the events table; reinstall to refresh.

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
  "summary": "Uninstalled browser-learning v0.1.0",
  "id": "browser-learning",
  "version": "0.1.0",
  "files": ["knowhow/browser-skills.md", "..."],
  "actor": { ... }
}
```

Both events are useful trigger sources. Examples worth considering:

- A `PluginInstalled` trigger that runs the new plugin's smoke test or pins its app to the launcher.
- A `PluginUninstalled` trigger that prompts the user "Want me to delete the listed files?" -- one workspace can wire this once instead of relying on the engine LLM to remember to offer it every time.

## Common mistakes to avoid

- **Building the archive with `tar`, `tar -czf`, or `gzip`.** The `.lucidos-plugin` extension is a renamed PKZip file, not a tarball. The install path opens it with `zip::ZipArchive::new()` (`engine/tools/plugins.rs::extract_zip`), which fails on gzip/tar with an opaque parse error. Always run `zip -r ../foo.lucidos-plugin .` from inside the plugin tree -- if `zip` isn't installed, install it (`brew install zip`, `apt install zip`) instead of substituting another archiver. Verify before handing the file off: `unzip -l foo.lucidos-plugin` should list the entries; `file foo.lucidos-plugin` should say `Zip archive data`, not `gzip compressed data`.
- **Calling the manifest `manifest.json`, `manifest.yaml`, or anything other than `manifest.toml`.** `validate_tree` looks for `manifest.toml` at the archive root and only parses TOML. Other names or formats reject the archive before any file is written. The required fields are `id`, `version`, `name`, `description` (optional: `source`, `engine`) -- see the schema table above.
- **Silently dropping or rewriting external references.** When an app references a file outside the plugin tree (`<img src="../../artifacts/foo.png">`, `<script src="/data/scripts/bar.js">`, etc.), do not guess. List every external reference back to the user and ask whether to bundle the file into the plugin tree, leave the reference as-is (and document the external dependency), drop the reference + dependent feature, or abort packaging. Auto-bundling without asking risks shipping the user's private workspace artifacts; auto-dropping risks publishing a plugin with a broken feature the user did not realise was lost. See "Authoring loop" step 2 for the full handling rule.
- **Putting a README at the plugin root.** Validation rejects any top-level entry that is not `manifest.toml` or one of the four content directories. Put your README inside `apps/<id>/` if it is app-specific, or only in the source repo (which is not part of what gets installed).
- **Using underscores or capitals in `id`.** `browser_learning` and `Browser-Learning` both fail validation. Stick to `[a-z0-9-]+`.
- **Setting `source` to a `.lucidos-plugin` path.** When `source` is present it must be a git URL -- a local archive path is not valid. If you're distributing as an archive only, just omit `source` entirely.
- **Forgetting to bump `version` before publishing a fix.** Existing installers will see `"Already at latest"` and never pick up your change.
- **Designing a plugin that overwrites user-editable files.** If the user is meant to edit `knowhow/sites/linkedin.com/selectors.md` after install, then your update flow either overwrites their edits or fails the conflict scan. Either ship the file as a starting template the user moves elsewhere, or document that updates require re-doing local edits.
- **Cross-plugin path collisions.** Two plugins shipping the same `data/` path race -- the second install wins, and uninstalling either one suggests deleting the shared file. Namespace your files (e.g. `knowhow/<plugin-id>-<topic>.md` or under a dedicated subdirectory).
- **Treating uninstall as destructive.** It is guide-only. Do not assume your files are gone after `PluginUninstalled` fires -- they may still be on disk, possibly edited.
