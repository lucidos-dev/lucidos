---
name: Building a Plugin
description: Use when the user wants to author, package, publish, share, install, update, or uninstall a Lucidos plugin -- phrases like "build a plugin", "publish a plugin", "share this app as a plugin", "package this for other workspaces", "ship a knowhow bundle", "install the X plugin", "update plugins", "make a .lucidos-plugin". Covers the manifest schema, plugin layout, the four LLM tools, distribution shapes, and the v1 guide-only uninstall semantics.
---

# Building a Plugin

How to package a coherent bundle of workspace content (apps + knowhow + triggers + scripts) so another workspace can install it as a unit. The full v1 design lives in `docs/plans/2026-04-29-plugins-v1-design.md` -- this file is the operational reference for authoring, distributing, and maintaining one.

## When a plugin is the right artifact

A plugin is one bundle other workspaces install as a single unit. Use it only when the answer to "is this a coherent thing someone else would benefit from installing whole?" is yes.

| You want to share... | Right answer |
|---|---|
| One app the user pinned and likes | App, copy/paste the `apps/<id>/` tree -- no plugin needed for a single file or two |
| A standalone knowhow file | Knowhow file, paste it into `data/knowhow/` -- knowhow is already portable |
| An app + the knowhow it relies on + the trigger that drives it | Plugin -- the pieces only make sense together |
| A self-healing browser-skills loop (write-side knowhow + read-side reflection knowhow that a trigger calls) | Plugin -- canonical example, see `lucidos-dev/plugins/browser-learning` |
| Engine defaults every workspace should always have | Not a plugin -- belongs in `system-knowhow/` |

The distinguishing test: removing any one file from the bundle would leave the others non-functional or actively misleading. If the files don't cohere, ship them separately.

## Questions to settle with the user before bundling

A plugin is a published artifact other workspaces install -- get the shape right before scaffolding. Skip questions the user has already answered.

1. **What's the cohesive unit?** List the files you intend to bundle and confirm each one would be misleading or non-functional without the others. If the answer is "they're related but each works alone", ship them as separate apps / knowhow / triggers, not a plugin.
2. **`id` and `name`?** `id` must match `[a-z0-9-]+`, max 64 chars -- confirm the slug. `name` is the human title.
3. **Distribution shape?** Single-repo git URL, monorepo subpath, or local `.lucidos-plugin` archive. Drives whether `source` is set in the manifest and where the user will publish.
4. **Cron triggers, OAuth, personal data?** None of those ship in plugins (see "What doesn't belong in a plugin" below). If the bundle would benefit from a cron trigger, surface that in the manifest `description` so the install-time LLM can offer to set one up -- confirm with the user that this is the intended UX.

## Plugin layout

A plugin is a directory containing `manifest.toml` at the root plus a subset of four content directories. The directories mirror `data/` one-to-one -- whatever lives at `<plugin>/<dir>/...` lands at `<workspace>/data/<dir>/...` at install time.

```
my-plugin/
  manifest.toml          # required, at root
  apps/                  # optional, mirrors data/apps/
  knowhow/               # optional, mirrors data/knowhow/
  triggers/              # optional, mirrors data/triggers/
  scripts/               # optional, mirrors data/scripts/
```

Validation rules enforced at install (`core/plugins.rs::validate_tree` and `validate_archive_entry_path`). Any failure rejects the archive before any file is written:

- `manifest.toml` exists at the plugin root and parses.
- All required manifest fields are present (`id`, `version`, `name`, `description`).
- `id` matches `[a-z0-9-]+`, non-empty, max 64 chars. Uppercase, underscore, dot all reject.
- `version` parses as semver.
- `source`, when present, looks like a git remote: starts with `https://`, `http://`, `git@`, or ends in `.git`. Bare strings, `file://` URLs, and `.lucidos-plugin` paths are not valid. `source` is optional -- omit it for archive-only plugins shared peer-to-peer (Slack drop, USB stick, attachment). `update_plugin` and `check_plugin_updates` will refuse a sourceless plugin with an explanatory error, but install and uninstall work fine.
- Top-level entries are exactly `manifest.toml` plus a subset of `{apps, knowhow, triggers, scripts}`. No root README, no `LICENSE`, no `.git`, no `node_modules`, no `__MACOSX` (auto-injected by macOS Finder when zipping). Put per-plugin docs and license inside the plugin's own subtree if needed.
- At least one of `apps/`, `knowhow/`, `triggers/`, `scripts/` exists with at least one file. An empty `knowhow/` directory passes the top-level check but fails as `EmptyTree`.
- Hidden files (any path component starting with `.`) are silently skipped during the file walk -- `.DS_Store`, editor swap files, and friends do not get installed.
- No archive entry uses `..` or absolute paths (`/`, `\`) -- zip-slip protection.

The flat one-to-one mapping means there is no separate "install destination" question. If your file lives at `apps/foo/index.html` in the plugin, it lands at `data/apps/foo/index.html` in the workspace. Sub-trees (`triggers/foo/foo.md`, `apps/foo/sdk-prefs.js`) are preserved verbatim.

## `manifest.toml` schema

Four required fields, two optional. Unknown extra fields are accepted and round-trip into the `PluginInstalled` event payload's `manifest` so future additive fields stay compatible with old install records.

| Field | Required | Type | Notes |
|---|---|---|---|
| `id` | yes | string | `[a-z0-9-]+`, max 64 chars. Used as the install-record key, the event `aggregate_id`, and the argument to `update_plugin` / `uninstall_plugin`. |
| `version` | yes | string | Semver (`MAJOR.MINOR.PATCH`). `0.1.0`, `1.4.2-beta.1` both parse. |
| `name` | yes | string | Human-friendly title shown in install/uninstall messages. |
| `description` | yes | string | One-line summary. Free text. If your plugin pairs well with a cron trigger, mention it here (e.g. "Ask Lucidos to set up a daily reflection trigger after install") so the install-time LLM offers to wire one up -- see "What doesn't belong in a plugin" below. |
| `source` | no | string | Git remote URL where the plugin lives. Used by `check_plugin_updates` and `update_plugin` to re-fetch the manifest. Omit for archive-only sharing -- the plugin still installs and uninstalls correctly, but updates cannot be fetched (the update tools will return an explanatory error). When present, it must look like a git remote (`https://`, `http://`, `git@`, or ending in `.git`). |
| `engine` | no | string | Semver constraint (e.g. `">=0.5.0"`). Parsed and stored but the v1 install path does not enforce it -- use it as documentation for now. |

Worked example (`browser-learning/manifest.toml`):

```toml
id = "browser-learning"
version = "0.1.0"
name = "Browser Learning"
description = "Self-healing site knowhow for browser automation. Agents emit observations during tasks; a reflection recipe folds them into per-domain knowhow so the next agent visits with better priors."
source = "https://github.com/lucidos-dev/plugins/tree/main/browser-learning"
```

The `source` may be the GitHub tree URL the user copied from the address bar -- the install tool parses it back into a git remote + branch + subpath. For a single-repo plugin, use the bare git URL.

## What doesn't belong in a plugin

The four content directories make almost anything technically packageable, but apply judgment to `triggers/`. Apps, knowhow, and **event-driven (`on_event`) triggers** belong in plugins -- they are reference material or part of the plugin's own mechanism. An `on_event` trigger that reacts to events the plugin's apps/knowhow emit ships naturally inside `triggers/`: install lands the file under `data/triggers/...` and uninstall removes it like any other shipped content.

**Cron triggers, OAuth credentials, and personal data do not ship in plugins.** They are workspace state -- WHEN something runs on a clock, WHO owns the account, WHAT the user has accumulated. Cron triggers in particular get singled out -- four reasons:

1. **Cadence is user-specific.** Heavy users want it every 6h, light users weekly. Hardcoding `0 0 4 * * *` in the bundle makes that decision for them.
2. **The schedule is workspace state, not reference material.** A plugin shipping a cron entry is the equivalent of a library shipping a crontab line -- wrong layer. Knowhow is "how to do this well", cron triggers are "when I want it to happen".
3. **`PluginUninstalled` is guide-only.** If the install instructions create a cron trigger as a side effect (rather than as a file), it does not appear in the install record -- uninstall does not know to remove it, and the workspace ends up with an orphaned cron entry pointing at deleted knowhow. (Event triggers shipped as files in `triggers/` avoid this: the install record cleans them up alongside the rest of the plugin's tree.)
4. **Install-time prompt is the right UX.** When `install_plugin` lands the knowhow, the LLM tells the user *"This plugin works best with a reflection trigger. Want me to set one up? Daily at 4am is a good default."* Conversational, opinionated default, but the user owns the schedule.

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

1. **Lay out the tree.** Create `my-plugin/manifest.toml` and the content directories. Author content as if it were already installed -- knowhow files use the same frontmatter rules as any other knowhow (`system-knowhow/building-knowhow.md`), apps follow the app conventions (`system-knowhow/building-an-app.md`), triggers obey the intent-vs-procedure rule (`system-knowhow/building-a-trigger.md`).
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

## Events emitted

Two `SystemEvent` variants (`engine/event_bus.rs:244-261`). Both carry `actor: Option<MessageOrigin>`. Both have `aggregate()` returning `"plugin"`. The `aggregate_id` is the plugin's `id` field.

### `PluginInstalled`

Emitted on every successful install (including overwrites and updates -- the variant is reused, no separate "PluginUpdated"). Persisted (`payload` JSONB) shape:

```json
{
  "type": "PluginInstalled",
  "data": {
    "manifest": {
      "summary": "Installed Browser Learning v0.1.0 from github.com/lucidos-dev/plugins/tree/main/browser-learning",
      "manifest": { "id": "browser-learning", "version": "0.1.0", "name": "...", "description": "...", "source": "https://github.com/lucidos-dev/plugins/tree/main/browser-learning", "engine": "..." },
      "files": ["knowhow/browser-skills.md", "..."],
      "installed_at": "2026-04-29T12:34:56+00:00",
      "source_type": "git"
    },
    "files": ["knowhow/browser-skills.md", "knowhow/browser-knowhow-reflection.md"],
    "installed_at": "2026-04-29T12:34:56+00:00",
    "source_type": "git",
    "actor": { ... }
  }
}
```

The two outer wrappers come from how Lucidos persists `SystemEvent`: serde's `tag = "type", content = "data"` adds `{type, data}`, and `install_from_unpacked_with_bus` packs the raw manifest into a payload map under `manifest` before assigning that map to `SystemEvent::PluginInstalled.manifest`. Net effect: the raw manifest fields (`id`, `version`, `source`, ...) sit at `payload.data.manifest.manifest.*`. `InstalledRecord` reads them at that path; do the same in any new consumer.

`source_type` is `"git"` for both plain git URLs and GitHub tree URLs, `"archive"` for `.lucidos-plugin` installs. `files` is the same list at the top level (`payload.data.files`) and inside the nested `manifest` blob (`payload.data.manifest.files`) -- the nested copy is what `latest_install` reads when looking up "what files belong to this plugin?" for the uninstall guide.

> **Historical bug, fixed.** Earlier `InstalledRecord::source()` read `payload.manifest.source` -- two layers too shallow -- and silently returned `None`, surfacing as the misleading `"installed manifest is missing 'source' -- cannot fetch latest"` error from `check_plugin_updates` even when the source was recorded. The matching `aggregate_id()` derivation read `manifest.id` (also too shallow), which made every PluginInstalled row land with `aggregate_id = "unknown"` and broke `latest_install(pool, &id)` lookups. Both are fixed; the e2e tests in `engine/tools/plugins.rs` (the `e2e_*` cases) lock the round-trip in. Plugins installed before the fix may still have `aggregate_id = "unknown"` in the events table; reinstall to refresh.

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
