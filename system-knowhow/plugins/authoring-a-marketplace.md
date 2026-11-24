---
name: Plugins, Authoring a marketplace
description: Part of Plugins. Its index lists the sections it covers.
---

One part of `system-knowhow/plugins`, which lists every part.

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
- **The walk never follows a symlink.** A symlinked directory is not entered,
  and a symlinked `manifest.toml` does not make a plugin root. A tree-URL
  subpath that passes through a symlink fails with `subpath not found inside
  repo`. Commit real files, not links.
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
  what `plugins(action="check_updates")` / `plugins(action="update")` re-fetch after install,
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
one of whose **Base URLs** scopes the clone URL. It hands that one credential
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
`system-knowhow/triggers/running-an-existing-trigger-once-off-sch.md` § "On-disk trigger definition"). It holds `name`, `run`
(`intent` or `script`) and `on` (trigger subscriptions). It also holds the usual optional
fields (`app_id`, `go_to_review`, `group_id`, and `model` /
`reasoning_effort` to pin the intent to a specific chat model). Put any
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
- **Never grants anything.** A `coding_agent_grant` or `side_effect_grant` in a
  shipped file is ignored: install creates the trigger ungranted, and an update
  keeps the user's own answer. If the trigger needs a grant, the plugin's setup
  tells the user what to tick (`system-knowhow/triggers/grouping-triggers.md` § "Coding-agent grant" and
  § "Program grants").

The user sees the `trigger.toml` files in the install confirmation panel's file
list before confirming, so activation is never silent.

## What doesn't belong in a plugin

Apps, knowhow, and **event-driven (`on_event`) triggers** belong in plugins: they are reference material or part of the plugin's own mechanism. An `on_event` trigger ships as a `triggers/<slug>/trigger.toml` declaration (see "Shipping triggers" above).

**Nothing user- or machine-specific ships in a plugin.** Many workspaces install one plugin identically. Anything that differs per installer is workspace state, not plugin content: an account, a schedule, a client id, a path on someone's disk. Ship the generic code, and use the `setup` field to have the agent ask each installer for their own value. The test for a file: would it still be correct on a machine that is not yours?

**Cron triggers, OAuth credentials, and personal data do not ship in plugins.** They are workspace state: WHEN something runs on a clock, WHO owns the account, WHAT the user has accumulated. Cron triggers stay out for four reasons:

1. **Cadence is user-specific.** Heavy users want it every 6h, light users weekly. Hardcoding `0 0 4 * * *` in the bundle makes that decision for them.
2. **The schedule is workspace state, not reference material.** A plugin shipping a cron entry is like a library shipping a crontab line: wrong layer. Knowhow is "how to do this well", cron triggers are "when I want it to happen".
3. **Orphaned cron entries.** A cron trigger the install instructions create as a side effect (an agent calling `triggers(action="create")`) carries no plugin provenance. Uninstall leaves it in place, and so does an update that drops the knowhow it runs. Lucidos warns about it as an *affected trigger* (see "Affected triggers"), but the trigger still points at deleted knowhow until the user edits it. A shipped `trigger.toml` avoids this, because uninstall deletes exactly the triggers stamped with the plugin id.
4. **Install-time prompt is the right UX.** When `plugins(action="install")` lands the knowhow, the LLM tells the user *"This plugin works best with a reflection trigger. Want me to set one up? Daily at 4am is a good default."* Conversational, opinionated default, but the user owns the schedule.

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

This overrides the `__file__`-relative state path in `system-knowhow/triggers/actor.md` § "Scripts run in place". A plugin's scripts also run in place, so that section still explains where `__file__` points. Its advice to keep state beside the script suits a trigger the user owns. For a plugin, beside the script is inside a folder the plugin manages.

