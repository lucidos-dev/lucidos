---
name: Plugins, When a plugin is the right artifact
description: Part of Plugins. Its index lists the sections it covers.
---

One part of `system-knowhow/plugins`, which lists every part.

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
6. **Does `setup` put any shipped file outside `data/`?** A Claude Code skill that setup copies to `~/.claude/skills/<name>/` is the usual case. If so, tell the user that every later release changing such a file must raise `setup_revision`. That makes the update ask setup to run again (see "A release can ask for setup to run again"). Skip the question when setup writes only inside `data/`.

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

Install validates the tree (`crates/lucidos-core/src/core/plugins.rs::validate_tree` and `validate_archive_entry_path`). Any failure rejects the archive before any file is written:

- `manifest.toml` exists at the plugin root and parses.
- All required manifest fields are present (`id`, `version`, `name`, `description`).
- `id` matches `[a-z0-9-]+`, non-empty, max 64 chars. Uppercase, underscore, dot all reject.
- `version` parses as semver.
- `source`, when present, looks like a git remote: starts with `https://`, `http://`, `git@`, or ends in `.git`. Bare strings, `file://` URLs, and `.lucidos-plugin` paths are not valid. `source` is optional: omit it for archive-only plugins shared peer-to-peer (Slack drop, USB stick, attachment). `plugins(action="update")` and `plugins(action="check_updates")` refuse a sourceless plugin with an explanatory error. Install and uninstall work fine.
- Top-level entries are exactly `manifest.toml` plus a subset of `{apps, knowhow, triggers, scripts, auth-modules, themes, fonts, media}`. `media/` is never content: it holds *plugin media* and counts toward nothing below. No root README, no `LICENSE`, no `.git`, no `node_modules`, no `__MACOSX` (macOS Finder adds it when zipping). Put per-plugin docs and license inside the plugin's own subtree if needed. A `looks/` folder, the name themes had before ADR 0316, is refused with a hint to rename it `themes/`.
- At least one of `apps/`, `knowhow/`, `triggers/`, `scripts/`, `auth-modules/`, `themes/`, `fonts/` exists with at least one file. An empty `knowhow/` directory passes the top-level check but fails as `EmptyTree`.
- Every file under `themes/` is `<id>.json` and a valid theme, with an id no built-in theme uses (`InvalidTheme` otherwise). The rules are in `themes.md`, theme parts included. A part value past a cap, or a part on a protected surface, fails staging with the field, the rule and the limit (`dark.parts.chat-text.text-shadow: blur 2em is over the 0.6em cap.`).
- Every file under `fonts/` belongs to a valid workspace font: `fonts/<slug>/font.json` plus the font files it names (`InvalidFont` otherwise). A theme in the plugin may name a workspace font only if the plugin ships it. Install is refused when the plugin's new fonts would take the workspace past 100. The rules are in `workspace-fonts.md`.
- The file walk silently skips hidden files (any path component starting with `.`), such as `.DS_Store` and editor swap files.
- The file walk silently skips build output. It does not enter a directory named `node_modules`, `target`, `dist`, `build`, `out`, `__pycache__`, `venv`, `.venv`, `.next`, `.pytest_cache` or `.git`, at any depth. It also skips any `*.pyc` or `*.pyo` file. These files are machine-specific, so they never reach the install record or the uninstall list. A tree that holds only build output fails as `EmptyTree`.
- An app, trigger or font folder with one of those names (`apps/dist/`, `triggers/build/`, `fonts/out/`) fails as `ItemNamedLikeBuildOutput`, so install never drops a whole item silently. Deeper down, install drops such a folder without a warning, so do not put real content in one.
- Symbolic links are never followed. A file symlink is skipped and a dir symlink is not walked. A top-level symlink named as a content dir is rejected. A symlinked `manifest.toml` counts as missing, at install and when `update` or `check_updates` reads the remote version. A tree-URL subpath that passes through a symlink fails with `subpath not found inside repo`. This keeps a plugin from reaching a file outside its own tree.
- No archive entry uses `..` or absolute paths (`/`, `\`): zip-slip protection.

There is no separate install destination. `apps/foo/index.html` in the plugin lands at `data/apps/foo/index.html` in the workspace. Sub-trees (`triggers/foo/foo.md`, `apps/foo/sdk-prefs.js`) are preserved verbatim.

## `manifest.toml` schema

Four required fields, nine optional. Unknown extra fields are accepted and round-trip into the `PluginInstalled` event payload's `manifest`, so future additive fields stay compatible with old install records.

| Field | Required | Type | Notes |
|---|---|---|---|
| `id` | yes | string | `[a-z0-9-]+`, max 64 chars. Used as the install-record key, the event `aggregate_id`, and the canonical argument to `plugins(action="update")` / `plugins(action="uninstall")`. (`plugins(action="uninstall")` also accepts the manifest `name` or any `apps/<dir>` folder name the plugin owns, case-insensitive. It picks one if unambiguous, otherwise lists candidates.) |
| `version` | yes | string | Semver (`MAJOR.MINOR.PATCH`). `0.1.0`, `1.4.2-beta.1` both parse. |
| `name` | yes | string | Human-friendly title shown in install/uninstall messages. |
| `description` | yes | string | One-line summary. Free text. If your plugin pairs well with a cron trigger, mention it here (e.g. "Ask Lucidos to set up a daily reflection trigger after install"). The install-time LLM then offers to wire one up: see "What doesn't belong in a plugin" below. |
| `source` | no | string | Git remote URL where the plugin lives. Used by `plugins(action="check_updates")` and `plugins(action="update")` to re-fetch the manifest. Omit for archive-only sharing: the plugin still installs and uninstalls, but the update tools return an explanatory error. When present, it must look like a git remote (`https://`, `http://`, `git@`, or ending in `.git`). |
| `engine` | no | string | Semver requirement on the Lucidos release (e.g. `">=0.46.1"`). **Enforced at install and update**: see "The `engine` requirement" below. Omit it and any release installs the plugin, but Lucidos shows that it declares none. Declare it. |
| `setup` | no | string | Markdown wiring instructions, written **to the agent about what to do with the user**. It renders in the install confirmation panel before the user confirms, and on confirm it drives a spawned Lucidos Agent setup thread. The engine never interprets it. This is the only way to ship workspace state rather than a file, a webhook above all. See "Install confirmation panel" below and `plugin-setup.md`. |
| `setup_revision` | no | positive integer | Raise it on a release that needs `setup` to run again although its text is unchanged. An update re-runs setup when this is **greater** than the installed version's. A missing field counts as 0. Needs a non-empty `setup`. See "A release can ask for setup to run again". |
| `setup_reason` | no | string | One line, at most 200 characters, saying why this revision asks for a re-run. Needs `setup_revision`. It shows in the confirmation panel and in the setup thread's first message. |
| `icon` | no | string | The plugin icon, a path inside `media/` (`"media/icon.svg"`). See "Plugin media". |
| `screenshots` | no | array of string | Paths inside `media/`, in the order the plugin detail page shows them. |
| `videos` | no | array of string | Paths inside `media/`, shown after the screenshots. |
| `categories` | no | array of string | Topical tags for browsing the **Store** (the Plugins panel's category filter). A **controlled vocabulary** (see below): pick from the allowed set. Normalised to lowercase on parse. An unknown value is **dropped and flagged** at catalog-scan time (it appears in the catalog's `errors`), never blocking install. |

**Plugin categories: the controlled vocabulary.** The allowed values are: `productivity`, `finance`, `health`, `developer-tools`, `data`, `communication`, `automation`, `lifestyle`, `research`, `fun` (kebab-case). The catalog offers a filter pill per category that appears in it, and each card shows its category chips. The set stays small so categories stay browsable: a free-form tag would fragment (`finance` vs `money` vs `budgeting`). Tag a plugin with the one or few that fit, or omit `categories` if none do. (Source of truth: `PLUGIN_CATEGORIES` in `crates/lucidos-core/src/core/plugins.rs`.)

**The `engine` requirement.** Set `engine` to the **first Lucidos release that has every platform feature your plugin uses**, as a floor: `engine = ">=0.46.1"`. For example, a plugin that styles a field with the shared `.text-input` class needs 0.46.1, the first release that ships it. The engine checks it every time a plugin is staged: the `plugins(action="install")` and `plugins(action="update")` tools, and the Plugins panel's Install and Update buttons.

- **Met**: staging goes ahead as usual.
- **Not met**: staging is refused before anything is written and before the confirmation panel opens. The refusal names the plugin, the requirement and the running release: `Theme Studio 0.1.0 needs Lucidos 0.46.1 or later. This is Lucidos 0.46.0. Update Lucidos first.`
- **Not a valid requirement** (a typo such as `"latest"`, an empty string, or a number instead of a string): refused the same way, with the bad value quoted. A malformed requirement never installs.
- **Release unreadable**: if Lucidos cannot read its own release, it refuses any plugin that declares `engine`, since nothing can vouch for it. A plugin without `engine` still installs.
- **Not declared**: the plugin installs and updates on any release, exactly as before. Nothing is refused, disabled or asked. Lucidos only shows the gap:
  - The confirmation panel carries a quiet note under the source: "This plugin doesn't say which Lucidos version it needs."
  - Until it is installed, the plugin detail page carries a muted "No version requirement" chip, with the same sentence as its tooltip.

**Always declare `engine`.** Without it, an older release installs the plugin, and it breaks at run time with no clear message. For example, an app that calls `lucidos.request` needs 0.39.0 or later, the first release that ships it. Declared, the same install is refused up front, with a sentence that tells the user to update Lucidos. Your floor is the release that first shipped the newest platform feature your plugin uses.

**The workspace audit keeps asking until you declare one.** `system-knowhow/workspace-audit/plugins.md` checks every installed plugin and every plugin tree a workspace authors. It flags a plugin with no `engine` on every pass, with a fix the user can act on: update to a version that declares one, ask the author, or add the floor themselves.

**Adding `engine` later still needs a version bump.** `plugins(action="check_updates")` compares semver. Add `engine` without raising `version`, and every existing installer sees `Already at latest` and never receives the floor.

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

- **Screenshots and videos show made-up demo data only, never the user's real data.** Media goes public with the plugin. So it shows no real thread titles, costs, names, scores, messages or files. "Authoring loop" step 2 has the procedure.
- **Paths start with `media/`.** Lucidos drops a path outside it, a symlink, a missing file, a wrong format and a file over its limit. It also drops an entry past a count limit, or one that takes the total over 64 MB.
- **A media problem never blocks.** The scan lists the plugin and install goes ahead. The plugin detail page names each dropped file and the limit it broke, so the author sees it in their own Plugins panel.
- **No `icon`?** The Plugins panel falls back to the app icon of the plugin's single app (widgets do not count), then to a *monogram tile*.
- **`media/README.md` is the long description.** It renders as markdown on the plugin detail page, through the same sanitizer as chat, so raw HTML and scripts are stripped. The manifest `description` stays the one line the list row shows.
- **Where it is kept.** A marketplace scan keeps each listed version's media in `.lucidos/plugin-media/`, a cache the next scan rebuilds. Install copies the plugin's media to `data/plugin-media/<id>/`. It is engine-managed and gitignored, never in a commit, the file list or the "Modified" badge. It outlives the marketplace and is in every backup. An update replaces it whole, and uninstall deletes it.
- **Served by `GET /api/v1/plugins/media/...`**, the Lucidos shell only, never an app. It serves only what a kept copy lists: the manifest's entries, the README and the app icon fallback. Every answer carries `X-Content-Type-Options: nosniff` and `Content-Security-Policy: sandbox; default-src 'none'`. An SVG opened directly cannot run script.

