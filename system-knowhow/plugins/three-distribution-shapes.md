---
name: Plugins, Three distribution shapes
description: Part of Plugins. Its index lists the sections it covers.
---

One part of `system-knowhow/plugins`, which lists every part.

## Three distribution shapes

`plugins(action="install", source)` detects the shape by string format (`crates/lucidos-agents-api/src/engine/tools/plugins/source.rs::detect_source`).

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

**Do not use `tar`, `tar -czf`, `gzip`, or any non-zip format.** The custom extension does not change the format. The engine opens it with `zip::ZipArchive::new()` (`crates/lucidos-agents-api/src/engine/tools/plugins/source.rs::extract_zip`), which only understands PKZip. A gzipped tarball or raw gzip stream fails with an opaque "read archive: ..." parse error, and the user has to repackage. If `zip` is not installed, install it (`brew install zip`, `apt install zip`) rather than substituting another archiver.

Install URL: an absolute filesystem path ending in `.lucidos-plugin` (`/Users/me/Downloads/my-plugin.lucidos-plugin`). The engine extracts the zip into a temp dir and validates as if it were a git checkout.

Pick this for ad-hoc sharing (Slack, email), pre-publication testing, and plugins that should not go to a public git host. The custom extension makes the file self-announcing and leaves room for OS file association later. Archive plugins may omit `source`: they install and uninstall normally, but `plugins(action="check_updates")` / `plugins(action="update")` report nowhere to fetch from. For updates while distributing as an archive, set `source` to the git repo the archive is built from.

## Authoring loop

1. **Lay out the tree.** Create `my-plugin/manifest.toml` and the content directories. Author content as if it were already installed:
   - knowhow files use the same frontmatter rules as any other knowhow (`system-knowhow/building-knowhow.md`);
   - apps follow the app conventions (`system-knowhow/building-an-app.md`);
   - triggers obey the intent-vs-procedure rule (`system-knowhow/triggers.md`).
2. **Add the plugin media.** The Plugins panel shows it before anyone installs, so treat it as part of the plugin:
   - **Icon**: write `media/icon.svg`, a simple shape on the app's own colours. With no icon, the panel shows the single app's icon or a *monogram tile*.
   - **Screenshots**: take them yourself. Never ask the user to open the app or to capture anything.
     - **Pick the device.** Use the phone from Known devices, because phone-width views read best in the detail page's strip. If Lucidos is not visible on the phone, use any device where it is visible. Ask the user only when no device is visible at all.
     - **Stage demo data first.** Screenshots and videos go public, so they show made-up demo data only. Never show real thread titles, costs, names, scores, messages or files.
       1. Back up each live file the capture touches, to a scratch folder outside `data/`.
       2. Write demo data into those files.
       3. Capture the views (the steps below).
       4. Restore each file byte for byte from its backup, and compare the two with `cmp`.
       5. Open each view again and verify that it shows the live data.

       If you cannot stage a view with demo data, leave that shot out and tell the user why.
     - **Tell the user first.** Each capture takes over the screen on their device for a moment. Do one capture at a time, and never capture from two threads at once.
     - **Capture each view.** Call `navigate_ui(target="app", app_id=..., device=<id>)` to open the view. Then call `capture_app(app_id=..., device=<id>, save_as_artifact="artifacts/plugin-shots/board.png")`. Pass `fragment` to `navigate_ui` when the app routes by hash. Do this again for each view worth showing.
     - **If the capture finds no app**, the device does not show this thread, so it got a toast instead of the view. Capture on the turn's last used device instead, which shows this thread.
     - **Copy the files into `media/`.** Each file is sharp, at the device's pixel ratio, in the format the extension names. Prefer `.png`, and use `.jpg` for a view over the 4 MB limit.
   - **Videos**: the same demo data rule and procedure apply.
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
4. **Bump `version` in `manifest.toml` before publishing.** Without a bump, `plugins(action="check_updates")` reports `"Already at latest"` to existing installers, and they never get the new content. Adding `engine` for the first time needs its own bump too (see "The `engine` requirement" above).
5. **Decide whether this release asks setup to run again.** Do this on every release of a plugin that has a `setup` field. See "A release can ask for setup to run again".
   - List the files this release changed.
   - Raise `setup_revision` by one when `setup` installs, copies, registers or builds any of them outside `data/`. A Claude Code skill that setup copies to `~/.claude/skills/<name>/` is the usual case.
   - Write `setup_reason` naming what changed, in one line.
   - Otherwise leave both fields alone. Never reword `setup` to force a re-run.
   - Tell the user which way you decided, and why.
6. **For archive distribution, package as zip and verify.** From inside the plugin tree, run `zip -r ../my-plugin.lucidos-plugin .`. Then check that `unzip -l ../my-plugin.lucidos-plugin` lists every expected file. `file ../my-plugin.lucidos-plugin` must report `Zip archive data`, not `gzip compressed data`.
7. **Commit and push.** For monorepo plugins, the install URL changes only if the subpath changes. `plugins(action="update")` re-fetches new content under the same path.

Authors need no manual smoke test: the engine's e2e tests cover install, update and uninstall.

`plugins(action="install")` takes only `source`. Installing over an existing tree stages
a confirmation panel listing the files it would overwrite. The user confirms
that panel before extraction writes to `data/`. Confirmation replaces each file
atomically through a temporary file and rename. A validation failure changes nothing.

A disk failure during extract (out-of-space, permission denied) returns an error mid-write but does NOT roll back files already written. No install record is emitted, so a failed install can leave a partial set of files on disk.

## Versioning and updates

Semver is enforced at parse time: `0.1.0`, `1.4.2-beta.1` parse, `latest` and `1.0` do not.

**A version bump alone does not re-run setup.** An update re-runs it when the new `setup` text differs, or when `setup_revision` is greater than the installed one. Raise the revision on a release that changes a file setup installs outside `data/`. See "A release can ask for setup to run again".

`plugins(action="check_updates", id?)` (`crates/lucidos-agents-api/src/engine/tools/plugins/registry.rs::check_plugin_updates_impl`):

- With `id` omitted, surveys every currently-installed plugin (newest `PluginInstalled` event for each `aggregate_id`, skipped if a later `PluginUninstalled` exists).
- For each plugin, fetches the manifest from the recorded `manifest.source` (shallow clone to temp, read `manifest.toml`, discard).
- Compares semver. `changed: true` only when the remote version is strictly greater.
- Checks the remote manifest's `engine` requirement. It reports `engine_requirement` (as authored, or `null` when undeclared), `engine_compatible`, and, when it is false, `engine_incompatible_reason` with the same sentence `plugins(action="update")` would refuse with. `changed: true` with `engine_compatible: false` means a newer version exists that this release cannot install yet.
- A network failure for one plugin becomes an `error` entry in the JSON output. It does not abort the whole check.

```json
[
  { "id": "browser-learning", "installed_version": "0.1.0", "latest_version": "0.2.0", "changed": true, "engine_requirement": null, "engine_compatible": true, "engine_incompatible_reason": null, "source": "...", "remote_manifest": { ... } },
  { "id": "theme-studio", "installed_version": "0.1.0", "latest_version": "0.2.0", "changed": true, "engine_requirement": ">=0.99.0", "engine_compatible": false, "engine_incompatible_reason": "Theme Studio 0.2.0 needs Lucidos 0.99.0 or later. This is Lucidos 0.46.2. Update Lucidos first.", "source": "...", "remote_manifest": { ... } },
  { "id": "habit-tracker", "installed_version": "1.4.0", "latest_version": "1.4.0", "changed": false, "engine_requirement": null, "engine_compatible": true, "engine_incompatible_reason": null, "source": "..." },
  { "id": "weather-feed", "installed_version": "0.3.0", "source": "...", "error": "fetch failed: ..." }
]
```

`plugins(action="update", id)` (`crates/lucidos-agents-api/src/engine/tools/plugins/mod.rs::stage_install_request`):

- Looks up the newest `PluginInstalled` for `id` (must not be followed by `PluginUninstalled`).
- Re-fetches the remote manifest from the recorded `source`.
- If `remote_version <= installed_version`, returns `Already at latest (v<x>)`: a no-op that emits no event. `compare_versions` treats remote-older-than-installed as `AlreadyLatest`, so `plugins(action="update")` cannot downgrade.
- Otherwise stages an install confirmation using the recorded source.
  The panel lists overwrites and removals for the user to approve.
  An unmet `engine` requirement refuses staging. Confirmed updates emit
  `PluginInstalled`, just as confirmed installs do.

If the recorded manifest has no `source`, the update returns an error rather than guessing.

**An update removes the files the new version dropped.** The removal set is the previous install's recorded `files` minus what the new version ships. So a file you moved, say from `apps/<app>/knowhow/<topic>.md` to `knowhow/<topic>.md`, ends up only at its new path. The panel lists the removals before the user confirms. The confirm deletes them after writing the new files, prunes the folders that leaves empty, and records the deletion in the install commit. Five kinds of path are never removed:

- a path another installed plugin still records;
- a path the new version ships under another case, since a case-insensitive volume such as the macOS default sees one file;
- a path outside the seven content dirs;
- a `triggers/<slug>/trigger.toml`, which the trigger re-sync deletes together with the trigger (see "Shipping triggers");
- a file already gone from disk.

Only recorded paths are candidates, so a file the user added beside a dropped one stays, and so does its folder. A dropped file the user edited is saved aside before it goes (see "An update keeps your changes").

### Affected triggers

An *affected trigger* is one the plugin does not own that relies on a path its update or uninstall removes. A trigger made with `triggers(action="create")` during setup is the usual case: it carries no plugin id, so the update leaves it as it is. Lucidos matches a trigger to a removed path in three ways:

| Reason (`reason`) | When it matches |
|---|---|
| `named-in-run` | The intent text or script path names the path, with or without `data/` or `.md`. A `load_knowhow` id counts only when it has a slash. |
| `owning-app` | The path is knowhow under the trigger's own app, `apps/<app_id>/knowhow/`. |
| `trigger-knowhow` | The path is knowhow under the trigger's own `triggers/<slug>/knowhow/` folder. |

A match is exact. `x.md` does not match `x-v2.md`, a one-word knowhow id is not a match, and an app asset such as `apps/<app>/assets/icon.svg` never matches by app ownership. A trigger the acting plugin owns is never reported, because the re-sync handles it. A trigger another plugin owns is.

**A match is a warning only.** The engine never edits, pauses or deletes an affected trigger, and the match never blocks the confirm. Four surfaces carry the warning:

- the panel, before the user confirms;
- the receipt, under **Triggers to check**;
- one notification per confirm, with no web push. Its tap opens the trigger, or the trigger list when there are several. The title reads `<Plugin> update removed know-how a trigger uses`, or `Removing <Plugin> removed know-how a trigger uses`;
- each later fire of the trigger, as a `[REMOVED KNOW-HOW]` block (see `system-knowhow/triggers/grouping-triggers.md` § "Know-how a plugin removed").

These warnings navigate to triggers; they do not offer a snapshot restore action.
Undo restore belongs to the snapshot completion notification and requires its own owner confirmation.

The confirm matches against the paths it actually deleted, with the same function the staging used. So a trigger made between staging and confirm still appears in the event.

A version that fails to parse (`compare_versions` with garbage on either side) counts as needing an update. The engine prefers attempting the install over silently doing nothing on corrupted data.

## Uninstall semantics

Uninstall stages a confirmation panel, just like install, and deletes nothing until the user confirms. The `plugins(action="uninstall", id)` tool (`prepare_uninstall_plugin`) and the Plugins panel's **Uninstall** button (`stage_uninstall_request`) open the same panel. Staging:

- Resolves `id` (or the manifest `name`, or an owned `apps/<dir>` folder) and reads the newest `PluginInstalled` record.
- Splits the recorded files into `files_present` and `files_missing`, and lists both in the panel. A recorded path outside the seven content dirs counts as missing, so the panel never offers it for deletion.
- Expires after 1 hour, like an install.

**Confirm** (`confirm_pending_uninstall`, then `uninstall_with_bus`):

1. Deletes each present file under `data/` and prunes empty parent folders. A content-dir root such as `data/apps/` always stays.
2. Commits the deletion in one commit, `"Uninstall plugin: <id> v<version>"`.
3. Emits `PluginUninstalled` with `files`, `files_deleted` and `files_missing`. A file that vanished between staging and confirm goes into `files_missing`.
4. Deletes the triggers stamped with this plugin's id, and reloads WASM signers if an `auth-modules/` file went.

The panel also lists every affected trigger under **Triggers that use these files**, from the staged `affected_triggers` (see "Affected triggers"). Uninstall leaves those triggers in place.

The confirm response carries `files_deleted`, `files_missing`, `affected_triggers` (omitted when empty) and a `summary` such as `Uninstalled Browser Learning v0.1.0 (2 files removed).` **Cancel** drops the staging and emits `PluginUninstallCanceled`.

What this means for plugin authors:

- **Uninstall deletes edited files too.** Confirming removes every recorded file still on disk, local edits included. The panel lists them first, and the workspace git history keeps each committed version. If your plugin invites edits, say so in its `description`.
- **Sharing a path between plugins is allowed but messy.** If two plugins both ship `knowhow/sites/linkedin.com/selectors.md`, whichever installs second wins (overwrite). Uninstalling either one deletes the file, even though the other plugin still relies on it. Namespace your files, e.g. `knowhow/<plugin-id>-<topic>.md` or a dedicated subdirectory.
- **Reinstall after uninstall is supported.** `plugins(action="install")` on the same source after `plugins(action="uninstall")` works: the engine treats the uninstall as a tombstone, and the next install is fresh.

