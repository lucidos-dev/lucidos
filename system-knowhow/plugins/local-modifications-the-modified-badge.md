---
name: Plugins, Local modifications (the "Modified" badge)
description: Part of Plugins. Its index lists the sections it covers.
---

One part of `system-knowhow/plugins`, which lists every part.

## Local modifications (the "Modified" badge)

A plugin's shipped content lives under `data/` like any other artifact. So the user, the Lucidos Agent or a coding-agent thread can edit it after install. When that happens the plugin's detail page shows a **Modified** badge. An update merges those changes into the new version where it can (see "An update keeps your changes" below).

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
| `removed` | The new version no longer ships the file. Saves yours aside, then deletes it. |

**A `removed` file is never kept in place, whatever the Keep my edits control says.** A stale copy left behind keeps loading, which is the failure the removal exists to prevent. A dropped file counts as unedited only when the install commit holds its exact bytes. A legacy install record has no install commit, so no edit can be ruled out. Every dropped file it still has on disk is then treated as `removed`, and saved aside whole, without a patch.

**A file already identical to the new version is not listed at all.** That is the common case after your own edit was accepted upstream. Those paths are written as shipped, with no row, no count, and nothing saved aside. The comparison is by content hash, so an oversized file is judged like any other.

The panel also carries one **Keep my edits** control, on by default. Clearing it takes a clean update: every file becomes `replaced`, apart from `restored` and `removed` ones, and every edit is saved aside. The choice reaches the engine as `?keep_local_changes=false` on the confirm. An absent flag means keep, so a caller that never showed the control cannot silently discard a patch.

**A discarded edit is never simply deleted.** Before anything is overwritten, the engine writes your version and a `.patch` of it under `data/artifacts/plugin-local-changes/<plugin-id>/v<version>/`, with a `README.md` explaining the folder. That root is git-tracked and never auto-deleted. A clean merge saves nothing, because your edit survives in the file itself.

**The panel never opens blind.** Staging reads every installed plugin's install commit from the event log. When that read fails, the engine cannot tell which files you edited. So staging refuses any install that would overwrite an existing file, and tells you to try again. A fresh install has nothing to lose and stages as usual.

**An edit made after staging refuses the confirm.** The engine hashes every edited file, and every file it would remove, when it stages. If one of them changed by the time you confirm, the confirm refuses and asks you to run the update again. Nothing is overwritten or deleted on a judgment you did not see.

**So does an install record that moved.** The confirm re-reads the install records. It refuses when this plugin's record changed, say a newer version was confirmed first. It also refuses when another plugin now records a path the update would remove.

Installing one version twice is allowed, so a second save of the same version lands in `v<version>-2` rather than replacing the first. The engine never writes over a folder it already saved edits into.

**A conflict gets no conflict markers.** These files are LLM context: `knowhow/*.md` loads into the agent's prompt as instructions. A file containing `<<<<<<<` is a corrupted instruction the engine acts on. So upstream's coherent version wins on disk, and yours is preserved beside it.

**`trigger.toml` is never text-merged.** The engine rewrites it after every install, so its bytes never match what the plugin shipped, and every update would conflict. A changed trigger definition reports as `replaced`, and the shipped definition wins.

**The install commit stays a pristine copy of what the plugin shipped.** A merged path is recorded from upstream's bytes, not from the working tree (`core::commit_data_paths_with_overrides`). A second commit then records the merged tree plus any saved-aside copies. That lets a patch carry forward across updates. Otherwise the *next* update would read your patch as upstream's content, find no local modification, and drop it.

### Proposing your patch upstream

Whenever the Modified badge is up, the plugin detail page offers **Send changes to author**. The install receipt offers it too, right after an update that kept a patch. `POST /api/v1/plugins/propose-upstream` with `{ "id": "<plugin-id>" }` does three things and stops:

1. Derives the diff from the install commit to the working tree, over the plugin's edited paths. Since the install commit is pristine, this is a patch against the version you are actually on.
2. Writes it to `data/artifacts/plugin-local-changes/<plugin-id>/proposed-v<version>.patch`, commits it, and emits `ArtifactCreated`.
3. Spawns a Lucidos Agent thread seeded with the plugin name and the patch path, returning its `thread_id` so the caller can navigate there. The seed is attributed to the engine with the reason `plugin_upstream_proposal`, beside the device that asked (`system-knowhow/thread-events/wire-format-and-metadata.md` § Engine origins).

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

Three `engine::event_bus::SystemEvent` variants for the install lifecycle, plus the two cancel-audit ones. All carry `actor: Option<MessageOrigin>`, all have `aggregate()` returning `"plugin"`, and the `aggregate_id` is the plugin's `id` field.

### `PluginInstalled`

Emitted on every successful install, overwrites and updates included (there is no separate "PluginUpdated"). Persisted (`payload` JSONB) shape:

```json
{
  "type": "PluginInstalled",
  "data": {
    "id": "browser-learning",
    "manifest": {
      "summary": "Installed Browser Learning v0.1.0 from github.com/lucidos-dev/plugins/tree/main/browser-learning",
      "manifest": { "id": "browser-learning", "version": "0.1.0", "name": "...", "description": "...", "source": "https://github.com/lucidos-dev/plugins/tree/main/browser-learning", "engine": "...", "setup": "...", "setup_revision": 2, "setup_reason": "..." },
      "files": ["knowhow/browser-skills.md", "..."],
      "installed_at": "2026-04-29T12:34:56+00:00",
      "source_type": "git",
      "commit": "<workspace-repo sha of the install commit>",
      "setup_thread_id": "<uuid of the spawned setup thread, when one was spawned>"
    },
    "files": ["knowhow/browser-skills.md", "knowhow/browser-knowhow-reflection.md"],
    "files_removed": ["apps/browser-learning/knowhow/browser-skills.md"],
    "affected_triggers": [
      { "trigger_id": "<uuid>", "trigger_name": "Nightly browser reflection", "paths": [{ "path": "apps/browser-learning/knowhow/browser-skills.md", "reason": "owning-app" }] }
    ],
    "installed_at": "2026-04-29T12:34:56+00:00",
    "source_type": "git",
    "actor": { ... }
  }
}
```

`files_removed` (at `payload.data.files_removed`) lists the paths this update deleted because the new version dropped them (see "Versioning and updates"). It is omitted when empty, which covers every fresh install.

`affected_triggers` (at `payload.data.affected_triggers`) lists the affected triggers of those removed paths (see "Affected triggers"). It is omitted when empty. Rows from before the field carry none, so a fire never warns about their removals.

The two outer wrappers come from how Lucidos persists `engine::event_bus::SystemEvent`: serde's `tag = "type", content = "data"` adds `{type, data}`, and `install_from_unpacked_with_bus` packs the raw manifest into a payload map under `manifest` before assigning that map to `SystemEvent::PluginInstalled.manifest`. Net effect: the raw manifest fields (`version`, `source`, `setup`, ...) sit at `payload.data.manifest.manifest.*`. `InstalledRecord` reads them at that path; do the same in any new consumer. `setup_thread_id` sits one level up, at `payload.data.manifest.setup_thread_id`, because the engine records it rather than the author.

`id` is the exception, and deliberately so: it is at `payload.data.id`, where every sibling `Plugin*` frame carries it. It is still inside the manifest too. Rows written before the top-level field existed carry it only there, so a reader that must cover history falls back to `payload.data.manifest.manifest.id`.

**Two paths name the same value, and which one you write depends on who is reading.**

| You are… | Path to the plugin id | Why |
|---|---|---|
| reading an event with the `events` tool (`action=query`) | `payload.data.id` | You get the stored row, envelope and all. |
| writing a `condition` on a trigger or an `await_event`, or reading `TRIGGER_EVENT_PAYLOAD` in a script trigger | `id` | Both see the payload with the `type` / `data` envelope already stripped, so neither names those two keys. |

Same rule for anything deeper: `payload.data.manifest.manifest.version` when reading, `manifest.manifest.version` in a condition. Getting this wrong is silent at match time. So the engine checks every condition path against recent stored payloads when you subscribe. It names the real path when yours is in none of them. See `system-knowhow/triggers/writing-cron-expressions.md` § "What a condition can say".

`source_type` is `"git"` for both plain git URLs and GitHub tree URLs, `"archive"` for `.lucidos-plugin` installs.

`files` is the same list at the top level (`payload.data.files`) and inside the nested `manifest` blob (`payload.data.manifest.files`). `latest_install` reads the nested copy to find which files uninstall deletes.

`commit` (at `payload.data.manifest.commit`) is the workspace-repo sha of the "Install plugin: ..." commit. It is the baseline the Modified badge diffs against (see "Local modifications" above). Legacy rows installed before this field existed never show as modified.

Some old rows carry `aggregate_id = "unknown"`, from a since-fixed bug, and `plugins::registry::latest_install(pool, &id)` cannot find them. Reinstall the plugin to refresh.

### `PluginLocalChangesMerged`

Emitted right after the `PluginInstalled` it belongs to, and only when the install met at least one locally-edited file:

```json
{
  "id": "email-triage",
  "version": "0.3.0",
  "merged": ["knowhow/email-triage.md"],
  "conflicted": ["apps/email-triage/index.html"],
  "replaced": ["triggers/email-triage/trigger.toml"],
  "removed": ["knowhow/email-triage-old.md"],
  "saved_paths": [
    "artifacts/plugin-local-changes/email-triage/v0.3.0/apps/email-triage/index.html",
    "artifacts/plugin-local-changes/email-triage/v0.3.0/knowhow/email-triage-old.md"
  ],
  "commit": "<sha of the keep-local-changes commit>",
  "actor": { ... }
}
```

This one IS stored, unlike the Modified badge beside it, and the difference is the point. The badge is a pure function of git plus disk, so it can always be recomputed. A merge is not: once the two commits are written, nothing on disk says which files merged and which lost. `saved_paths` names the copies of every discarded edit, each with a `.patch` sibling when the edit is text and a baseline exists. Every list is omitted when empty.

### `PluginUninstalled`

```json
{
  "id": "browser-learning",
  "version": "0.1.0",
  "files": ["knowhow/browser-skills.md", "knowhow/browser-knowhow-reflection.md"],
  "files_deleted": ["knowhow/browser-skills.md"],
  "files_missing": ["knowhow/browser-knowhow-reflection.md"],
  "affected_triggers": [
    { "trigger_id": "<uuid>", "trigger_name": "Nightly browser reflection", "paths": [{ "path": "knowhow/browser-skills.md", "reason": "named-in-run" }] }
  ],
  "actor": { ... }
}
```

`files` is every recorded path, and `files_deleted` plus `files_missing` split it. Each list is omitted when empty. Rows from before uninstall deleted files carry neither. `affected_triggers` matches against `files_deleted` and is also omitted when empty.

Both events are useful trigger sources. Examples worth considering:

- A `PluginInstalled` trigger that runs the new plugin's smoke test or pins its app to the launcher.
- A `PluginUninstalled` trigger that offers to remove the plugin's runtime state under `data/artifacts/<plugin-id>/`, which uninstall leaves alone.

## Common mistakes to avoid

- **Building the archive with `tar`, `tar -czf`, or `gzip`.** A `.lucidos-plugin` is a renamed PKZip file. See "3. Local archive" and "Authoring loop" step 6.
- **Calling the manifest `manifest.json`, `manifest.yaml`, or anything other than `manifest.toml`.** `validate_tree` looks for `manifest.toml` at the archive root and only parses TOML. Other names or formats reject the archive before any file is written. See the schema table above for the fields.
- **Silently dropping or rewriting external references** (`<img src="../../artifacts/foo.png">`, `<script src="/data/scripts/bar.js">`). Ask the user about each one: see "Authoring loop" step 3.
- **Putting a README at the plugin root.** Validation rejects any top-level entry that is not `manifest.toml` or one of the eight content directories. Put your README inside `apps/<id>/` if it is app-specific, or only in the source repo, which install never reads.
- **Committing `__pycache__/` or `*.pyc` files.** Install skips them (see "Plugin layout"), but they still bloat the repo and add noise to every PR. Give the plugin repo a `.gitignore` that lists `__pycache__/`, `*.pyc` and any other build output your scripts or apps create.
- **Writing runtime state inside the plugin's own folders.** See "Where a plugin keeps its runtime state".
- **Using underscores or capitals in `id`.** `browser_learning` and `Browser-Learning` both fail validation. Stick to `[a-z0-9-]+`.
- **Setting `source` to a `.lucidos-plugin` path.** When present, `source` must be a git URL. For archive-only distribution, omit `source`.
- **Rewording `setup` to force a re-run.** That turns the instructions into a version stamp. Raise `setup_revision` instead.
- **Forgetting to raise `setup_revision` when a file setup installs changed.** The `data/` copy updates, but the copy outside `data/` stays stale and no setup thread starts.
- **Lowering or resetting `setup_revision`.** Only a greater value re-runs setup, so a lowered or removed one does nothing.
- **Forgetting to bump `version` before publishing a fix.** Existing installers see `"Already at latest"` and never get your change.
- **Expecting an update to keep every user edit.** An update merges edits, but a conflict, a binary, a `trigger.toml` or a file over 1 MB takes the new shipped version. The user's copy is saved aside (see "An update keeps your changes"). If the user is meant to edit `knowhow/sites/linkedin.com/selectors.md`, ship it as a template they copy elsewhere.
- **Cross-plugin path collisions.** See "Sharing a path between plugins" under "Uninstall semantics".
- **Keeping user data in a shipped file.** Confirming an uninstall deletes every recorded file still on disk, edits included. Keep runtime state under `data/artifacts/<plugin-id>/`, which uninstall never touches.

## Diagnostic evidence

Plugin lifecycle logs use the sanitized engine diagnostic log.
Legacy lines remain private when rendered for a maintainer.

Follow [Agent Sandbox](../agent-sandbox.md) for bounded diagnostic access and the raw-host-log restriction.
