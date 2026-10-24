---
name: Workspace File Conventions
description: Workspace file layout (artifacts/, apps/, knowhow/, intents/, scripts/, config/, env vars) and the type scale to paste into standalone HTML reports.
---

# Workspace File Conventions

How files and data are organized in a Lucidos workspace.

## artifacts/: User Data & Content

### Fixed directories
| Path | Purpose |
|------|---------|
| `user_profile.md` | Learned facts about the user. The agent writes confirmed facts the user shares. Background memory extraction never appends to it |
| `imported/{service}/` | Data from APIs or local filesystem (e.g., `imported/oura/`, `imported/weather/`) |
| `projects/{name}/` | Major project folders, each with `notes.md` and related files |
| `screenshots/` | Browser screenshots (auto-named with timestamp) |
| `research/` | Research documents, deep dives, technical analysis |
| `generated/` | AI-generated images and content (default location) |

### App data storage
Apps store persistent data in `artifacts/{app-id}/`:

| Folder | App | Contents |
|--------|-----|----------|
| `habits/` | Habit Tracker | `data.json` |
| `todo/` | Todo | `data.json` |
| `morning-dashboard/` | Morning Dashboard | `YYYY-MM-DD.json` |
| `google-docs/` | Google Docs | `state.json`, `cache.json` |

Name the folder after the app's ID. The app's knowhow documents the data format.

### Generated content
- Default: `artifacts/generated/`
- Themed collections get their own folder (e.g., `artifacts/fargeleggingsark/`)

### Standalone HTML: paste this type scale, never recall one

A design mockup, report or dashboard written to `artifacts/` is a **standalone document**. It links no Lucidos stylesheet, so it has no tokens unless you write them. A scale written from memory comes out too large: it assumes a 16px body. Body text here is 12px, the size chat renders at.

Paste this block verbatim, then use only these steps:

```css
:root {
  --font-size-3xs: 0.5625rem;   /* 9px  micro-label, tiny uppercase badge */
  --font-size-2xs: 0.625rem;    /* 10px dots, micro-meta */
  --font-size-xs: 0.6875rem;    /* 11px dense metadata */
  --font-size-sm: 0.75rem;      /* 12px BODY TEXT, the chat prose step */
  --font-size-md: 0.8125rem;    /* 13px labels, controls */
  --font-size-lg: 0.875rem;     /* 14px emphasis */
  --font-size-xl: 1rem;         /* 16px section headings */
  --font-size-2xl: 1.125rem;    /* 18px larger headings */
  --font-size-3xl: 1.25rem;     /* 20px large headings */
  --font-size-display: 2.25rem; /* 36px hero */
}
body { font-size: var(--font-size-sm); line-height: 1.5; }
input, textarea, select, button { font-family: inherit; font-size: inherit; }
```

Four common mistakes, most frequent first:

1. **`1rem` is a heading here** (`--font-size-xl`), not body. Body is `0.75rem`. A size you pick from habit is two steps too big.
2. **Do not bake the UI scale into the file** (`html { font-size: 125% }`, or `20px` "for the 125% scale"). The preview already zooms the document by the viewer's UI scale, so a baked one applies twice. It also pins one device's preference into a file the user opens on several. Leave the root alone and size in `rem`.
3. **Keep the body and control lines.** A `<code>`, a `<button>` and an `<input>` each carry their own UA font and inherit nothing. Without those lines the document mixes its font with the browser's.
4. **Every `font-size` reads a step.** No `0.9375rem`, no `1.05rem`, no `0.8rem`. If none of the ten fits, pick a different step, not a new number.

Use `--font-mono` for code. The UI font is whatever the user picked, so a mockup meant to look like Lucidos says `font-family: 'Fira Code', ui-monospace, SFMono-Regular, Menlo, monospace`.

### What a standalone HTML document can do

An HTML file under `artifacts/` is untrusted content: it may come from an upload or a fetched web page. So the preview runs it in a sandboxed frame at an opaque origin. The engine serves it the same way when it is opened on its own. Write for that:

| Works | Does not work |
|---|---|
| Its own `<script>`: sortable tables, charts, tabs | Calling the engine: `fetch('/api/v1/…')`, `EventSource`. The engine refuses the request. |
| Relative assets next to it, loaded as tags: `<img src="img/chart.png">`, `<link href="style.css">` | Reading a sibling file from script: `fetch('data.json')`. It is another origin, and CORS refuses it. |
| Links a reader clicks: a sibling file, `#section`, `thread:…`, `app:…`, `repo:…:file:…` | A link the script follows on its own. Only a real click navigates. |
| `<a href="results.csv" download>` in a browser, a Copy button, a video's fullscreen button | `localStorage`, `sessionStorage`, `IndexedDB`, cookies. An opaque origin has none. |
| `mailto:` and `tel:` links | Reading or changing the Lucidos window around it (`parent.…`) |

On a phone, a sideways drag anywhere in the preview swipes between Lucidos panes, as it does over an app. A page wider than the screen keeps its sideways drag, so it pans instead. A carousel or a canvas keeps its own drag by claiming it, the same way an app does (`system-knowhow/js-sdk.md` § Pane swipe).

A document that needs live workspace data, or must remember state between opens, is an **app**, not an artifact. An app reads the workspace through `lucidos.*` (`system-knowhow/js-sdk.md`), and remembers per-device state in `lucidos.storage`. If a snapshot is enough, write the data into the report, as a JSON `<script>` block or inline in the markup.

Relative assets load from `artifacts/` only. An HTML file elsewhere (`knowhow/`, an app folder) still renders, but behind the gateway its relative images and stylesheets do not load. Keep a report and its assets together under `artifacts/`.

## apps/: App UIs & Logic

Each app: `apps/{id}/`

| File/Dir | Purpose |
|----------|---------|
| `index.html` | App UI |
| `manifest.json` | Name, description, `icon` (a path to an image in the app folder; see `building-an-app.md` § App icon), and `reveal` (when the loading cover lifts; see `js-sdk.md` § Showing the app once its content is ready). User-facing, not loaded into the LLM |
| `knowhow/` | App-specific reference docs |
| `intents/` | App-specific user intents |
| `scripts/` | App-specific helper scripts |
| `assets/` | Static files used by the app UI (images, fonts, PDFs) |

**App assets** (images, brochures, icons) go in `apps/{id}/assets/`, not in `artifacts/`.

## knowhow/: Shared Domain Knowledge

Reusable reference docs for several apps or prompts:
- `knowhow/{domain}/`, e.g. `oura/`, `google-workspace/`
- Each file has a clear, descriptive name: `api-ref.md`, `lucidos-data-storage.md`, `data-format.md`

**Placement**: a knowhow doc used by one app goes in `apps/{id}/knowhow/`. One used by 2+ consumers goes in `knowhow/{domain}/`.

**Depth rule**: `knowhow/` lists `{name}.md` and `{domain}/{name}.md` as docs. An app's or a trigger's own `knowhow/` lists `{name}.md` only, since the app or the trigger is already the domain. A file deeper than that is one doc's reference: it stays loadable by full id, but nothing routes to it and the doc that owns it must name it. Put a doc's supporting files in a folder named after the doc. See `system-knowhow/building-knowhow.md` § "Where the file goes".

## intents/: User Intents

Intent definitions not tied to a single app. App-specific intents go in `apps/{id}/intents/`.

## scripts/: Shared Scripts

Helper scripts that intents, knowhow or proxy auth handshakes invoke, not tied to a single app:
- `scripts/{name}/run.py`
- App-specific scripts go in `apps/{id}/scripts/`

## config/: Engine Configuration

JSON files the engine reads:

| File | Purpose |
|------|---------|
| `config/apis.json` | API proxy entries: maps a name to a `base_url` (and optional `auth` referencing a stored credential, and an optional `timeout_secs`). Powers `lucidos proxy <name> ...` (CLI), `lucidos.proxy(name).fetch(...)` (SDK), and the `proxy_request` LLM tool. The builtin provider proxies (`openai`, `anthropic` and the other model providers) need no entry here, and an entry with the same name overrides one. See `system-knowhow/lucidos-cli.md` § `lucidos proxy` for the schema and `system-knowhow/js-sdk.md` § `lucidos.proxy` for the iframe-side API. |

**Scripts and apps should call external APIs this way.** Add an entry once, then call the backend by name everywhere. The credential never appears in script source, args, env vars, log lines or LLM tool transcripts. The pre-proxy pattern is drift (see the workspace audit): `curl -H "Authorization: Bearer $CRED_..."` in scripts, or `fetch` with the credential pasted into the iframe.

## Every subprocess call is a fresh process

`run_bash`, `run_bash_background`, `run_python` and `run_python_background` each spawn a **brand-new process**. Bash runs via `bash -o pipefail -c`, so a later stage never masks a failing one. **No shell state carries over between calls**: an `export VAR=…`, a `cd somewhere` and any shell functions are gone by the next call.

```bash
# call 1
export GWS_CONFIG_DIR=/Users/me/.config/gws-work   # set in this process only

# call 2: fresh process, the export above never happened
gws calendar list        # GWS_CONFIG_DIR is empty → wrong/no account
```

Fixes, in order of preference:

1. **Inline the env var on the same line as the command**: `GWS_CONFIG_DIR=/Users/me/.config/gws-work gws calendar list`. For `cd`, chain in one call: `cd /some/dir && ./run.sh`.
2. **Same value across all calls in the workspace?** Define an **environment variable** (next section), so every subprocess inherits it.

Exception: the engine sets `LUCIDOS_WORKSPACE` and `LUCIDOS_DATABASE_URL` in every subprocess, so each fresh call has them. Credentials are set per call, by name (next section).

## Credentials in subprocesses: name what you use

`run_bash`, `run_bash_background`, `run_python`, `run_python_background` and scheduled scripts get a credential's variable **only when their own text names it**. That is a *secret mention*: the variable as a whole word, in the command or code, or in a workspace `.py` / `.sh` file the command runs.

- **A script that reads `os.environ["CRED_GITHUB"]` works as is.** `python data/triggers/watch/fetch.py` gets what `fetch.py` names, with no edit to the command. The `.py` / `.sh` files beside it count too, so a `run.sh` calling `python3 fetch.py` works. A module imported from another directory does not: name its variables in the header comment below.
- **One mention sets the whole credential.** Naming `CRED_MAIL_USERNAME` also sets `CRED_MAIL_PASSWORD`.
- **A custom alias needs naming too.** A tool that reads `GITHUB_TOKEN` by itself, like `gh`, gets it from `gh pr list  # GITHUB_TOKEN`.
- **A name built at run time** needs a header comment: `# lucidos-credentials: CRED_A CRED_B`.
- **`OAUTH_<PROVIDER>_ACCESS_TOKEN`** follows the same rule. Only a named account is refreshed.
- **To see the names**, run `lucidos credentials list`. `env | grep CRED_` finds nothing, because a bare `CRED_` names no credential. The result carries a note listing the names instead.
- **An unknown name still runs.** The result carries the same note, with names only, never values.

Every subprocess output is masked: an injected value, a token in the engine's own environment, and well-known token shapes such as `ghp_…` come back as `[REDACTED]`. A token in the shell that started the engine never reaches a chat tool's environment. To use one there, store it as a credential.

## Listing this machine's workspaces

Read `system-knowhow/scripts/list-workspaces.sh` for the shipped helper.
It queries the gateway's registered inventory and health, including stopped
workspaces. A ports file records an assignment and proves no running process.

Pass two positional arguments: the loopback gateway origin and the local token
file path, normally `~/.lucidos/local-token`. The origin includes its scheme
and gateway port; see `system-knowhow/remote-access.md` for identifying it.
For a private HTTPS certificate, pass `--ca-file <trusted-ca.pem>` too.
The helper reads the token from disk and never places its value in argv.
Its output is a JSON-quoted name and health value, separated by a tab.

## The workspace database: `LUCIDOS_DATABASE_URL`

A script, trigger or agent shell reaches the workspace's own database through one variable, `LUCIDOS_DATABASE_URL`. Pass it explicitly:

```bash
psql "$LUCIDOS_DATABASE_URL" -At -c "SELECT count(*) FROM thread_summaries"
```

```python
import os, psycopg
conn = psycopg.connect(os.environ["LUCIDOS_DATABASE_URL"])
```

- **It is read-only by default.** Every transaction opens read-only, so an accidental `DROP`, `DELETE` or `CREATE` fails with `cannot execute … in a read-only transaction`. A deliberate write opts in per transaction: `BEGIN READ WRITE; …; COMMIT;`.
- **It cannot read stored secrets.** It signs in as an agent role with no access to secret columns, such as `credentials.auth_value` or OAuth tokens. A query naming one fails with `permission denied`. Reach a secret through its `CRED_*` variable or the proxy instead. Every other column reads as usual. How secrets are stored: `system-knowhow/stored-secrets`.
- **Never put the URL or a password on the command line.** The command is recorded in the tool call. Naming the variable keeps the password out, and tool output masks the URL's value.
- **Bare `psql` reaches nothing.** The engine sets no `PGHOST` / `PGUSER` / `PGDATABASE` defaults and passes on no `DATABASE_URL` of its own. So an unset variable, as in `psql "$STAGING_URL"`, fails instead of landing on the workspace database.
- **A project's own database is yours to configure.** Set its `DATABASE_URL` or `PG*` names as environment variables (next section); the engine no longer reserves them.

## Environment variables: Per-Workspace Config

For environment that must be the **same for every subprocess in this workspace**, define an **environment variable** (Settings → System → Environment variables). These are DB-backed, non-secret `NAME=value` pairs. The engine injects them as real env vars into every subprocess it spawns: `run_bash`, `run_python`, background tasks, scheduled scripts, triggers, and coding-agent (Claude Code / Codex) sessions.

- **You (the agent) can list, set and delete them** with the grouped `env_vars` tool (`action: list | set | delete`; `set`/`delete` take `name`, `set` also `value`). The retired `set_environment_variable` name still works as an alias for `set`. The user can also edit them in Settings. Changes take effect on the **next** tool call or agent turn, with **no engine restart**.
  - Exception: the engine's *own* shell-outs read their vars at startup. For example, the Apply-time `git push` uses `GIT_SSH_COMMAND` / `GH_CONFIG_DIR`. A change reaches the engine's own git on the next restart. Tool and agent subprocesses see it immediately.
- **Non-secret only.** Values appear in logs, the event store and tool-call payloads, by design. For API keys, tokens or passwords use a **credential** (`request_credential`). A call that names `CRED_<NAME>` gets it, and output masks its value.
- **Names** are uppercase letters, digits and underscores, not starting with a digit (e.g. `CLAUDE_CODE_USE_VERTEX`, `LUCIDOS_REPO`). Engine-owned names (`CRED_*`, `OAUTH_*`, `PATH`, internal `LUCIDOS_*` like `LUCIDOS_WORKSPACE` and `LUCIDOS_DATABASE_URL`) are rejected. Engine-owned vars always win a collision.

The main use is **per-workspace identity**: a `gh` config dir, a Google `gws` config dir and project id, an SSH command. Then `gh` / `git push` from agent subprocesses authenticate as the right account:

```
GH_CONFIG_DIR=/Users/me/.config/gh-work
GIT_SSH_COMMAND=ssh -i /Users/me/.ssh/id_work -o IdentitiesOnly=yes
```

Setup is **partly interactive**. You can set the variables, but the user must complete the auth handshake:

1. Pick a dedicated gh config dir and authenticate it once (user-run, opens a browser): `GH_CONFIG_DIR=<dir> gh auth login`.
2. For SSH push, make sure the key referenced by `GIT_SSH_COMMAND` is registered on that GitHub account.
3. Set `GH_CONFIG_DIR` and `GIT_SSH_COMMAND` with the `env_vars` tool (`action: set`), or in Settings → System → Environment variables.

**Want a credential's secret under a specific env var name?** Give the credential a custom env var name, for a CLI or SDK that expects an exact variable. Its secret then also injects as e.g. `GITHUB_TOKEN`, and the default `CRED_<NAME>` still works. A call gets it only when it names the alias or `CRED_<NAME>`. Set it two ways:

- In the credential editor (Settings → credential editor).
- Up front, when the agent requests the credential. `request_credential` takes an optional `env_var_name` arg that pre-fills the modal's "Env var name" field. The user can still edit or clear it before saving.

The name must match `[A-Z_][A-Z0-9_]*` and can't be an engine-owned name (`CRED_*`, `OAUTH_*`, `PATH`, `LUCIDOS_*`). It works for single-value auth types only. A `password` credential ignores it, since it splits into `_USERNAME`/`_PASSWORD`.

**An auth handshake script never gets the custom name.** It receives only `CRED_*` and `OAUTH_*` names, so it reads `CRED_<NAME>`. See `building-an-auth-handshake`.

**A credential names every host it may be sent to.** Its Base URLs field is a set, one row per hostname, and the secret is refused at any host outside it. So a provider with API, clone and download traffic on different hostnames takes **one** credential naming all of them. Nothing is inferred from a hostname's spelling: name each in full. `request_credential` takes the whole set in `base_urls`. Asking for a host an existing credential does not cover reopens that credential, so the user never types the same secret twice.

**A secret can stay in 1Password.** Any secret field in the credential editor has a **From 1Password** toggle. On, the field takes a *secret reference* (`op://vault/item/field`) instead of the value. Lucidos reads it through the `op` tool with a service account token, set up under **Settings → Accounts → 1Password**. You cannot see or set the token.

Nothing changes for a script or app: it still names `CRED_<NAME>` or calls the proxy. When the value cannot be read, the call fails or the variable is left out, and the result says why. Point the user at 1Password or that Settings section, not at the script.

**Every secret the user enters is a credential.** A mailbox password is the `email_password` credential. A secret MCP server env value belongs in a credential too: `setup_mcp_server` takes `{"credential": "<name>"}` as that variable's value. Removing the server deletes a credential named `mcp-<server id>-<VAR>` that no other server names. A credential with any other name stays.

The legacy `data/.env` file is retired. On the next engine startup, any existing `data/.env` moves into the environment-variables store and the file is removed.

## Key Rules

1. **Never nest artifacts**: `artifacts/artifacts/` is always wrong
2. **App data** → `artifacts/{app-id}/`
3. **App assets** → `apps/{id}/assets/`
4. **Imported data** → `artifacts/imported/{service}/`
5. **Generated content** → `artifacts/generated/` (or themed subfolder if a pattern emerges)
6. **Research** → `artifacts/research/`
7. **One source of truth**: don't duplicate files across locations
8. **Import the minimum.** Never dump a whole repo, dataset or archive into `artifacts/imported/` to grab one or two files. Artifact count is a performance axis: every extra file inflates linkify, file lists, scans and per-render paths.
   - **Cloning a repo to inspect, run or extract from it** → clone into `.lucidos/tmp/{repo-name}/` (ephemeral, gitignored, not counted). Inspect it with `read_file`. Then `copy_file` only the files the app needs into `artifacts/imported/{service}/`; it reads a `.lucidos/tmp/` source directly. If the user wants the full repo to persist (e.g. to keep editing it), ASK first where to put it. Never decide on your own to dump it under `artifacts/`.
   - **Bulk datasets and archives** (Wikifonia-style: thousands of files, of which you use a few) → same rule. Inspect under `.lucidos/tmp/` and extract the entries the app uses into `artifacts/imported/{service}/`. Leave the bulk archive out unless the user says "keep the whole archive available".
   - **Bulk reference corpora the user wants to keep, but not in the workspace** → `~/.lucidos/data/{name}/` (sibling to `~/.lucidos/knowhow/`, cross-workspace, persistent, agent-discoverable). Pin the absolute path in the app's knowhow so converter scripts find it. `lucidos data-store add {name} {source-dir}` moves an existing directory there.
   - **Intermediate, debug or one-shot render output** (e.g. cropping tiles, OMR debug pixmaps, scratch PNGs from a one-time analysis) never goes in `artifacts/imported/`. Use `.lucidos/tmp/` and delete it after the analysis.
   - **Inherited cruft from earlier sessions**: unexplained files under `data/artifacts/imported/` that the consuming app's source, scripts and knowhow never mention. Verify each one by grep, and ask the user about ambiguous cases. Then `git rm` the dead files in one commit, with before/after artifact counts in the message.
9. **Chained edits use post-edit content.** When you `edit_file` the same file twice in one turn, build the second `old_string` from what the first call returned, not from an earlier read. Otherwise the call fails with "The file was likely modified by a previous edit". A file needing 3+ edits in one turn takes one `write_file` rewrite instead. Every text-mode edit answers `N of M occurrences replaced`: read both numbers. `N < M` leaves the rest in place, which is right only if you meant the first one; `replace_all: true` takes the rest.
10. **Where the file tools can reach.** Three prefixes resolve, plus a registered repository by name. Everything else is rewritten or refused:
    - `data/` (the default): an untyped path like `notes.md` becomes `artifacts/notes.md`. Readable and writable.
    - `.lucidos/tmp/…`: the ephemeral scratch tree, workspace-root-relative, gitignored. **Readable** by `read_file` and as a `copy_file` source. That is how you pull a file out of a `git_clone` tmp checkout, or read what `http_request(temp_path)` saved. **Not writable**: the file tools git-commit everything they write, so `write_file` / `edit_file` / `delete_file` refuse it. Create scratch with `run_python` (cwd is the workspace root), delete it with `run_bash`.
    - `system-knowhow/…`: engine-shipped reference docs. Read-only.
    - A **registered repository**, via the `repo` argument on `read_file`, `glob_files`, `grep_files` and `edit_file`. Pass its name or id (`manage_repositories` action `list`), and the path or pattern becomes repo-root-relative, over the working tree. This is how you work on code. Never `cd` into a checkout to `cat`, `sed`, `grep` or `awk` it: that returns raw untruncated bytes and walks `target/`. The file tools chunk, slice by line, cap the match set, and see only what git sees. Gitignored files stay invisible, so a build log still needs `run_bash`.

    **Editing a repository commits nothing.** `edit_file` needs `commit: false` there, and `write_file` / `delete_file` / `copy_file` refuse `repo` outright. The edit lands only in the working tree. Say so when you report it, and point the user at `git diff`. Use `run_coding_agent` instead when the work adds or deletes files, or should arrive as a change the user reviews and applies.

    Everything else under `.lucidos/` is engine runtime state (`worktrees/`, `exhaust/`, `engine.pid`) and is refused in both directions. `..` and absolute paths are rejected outright. For a path outside the workspace that is not a registered repository, use `run_bash` (`sed -n`, `cat`).

## Images posted in a thread

Every image a user pastes (and every image you generate) has two addresses. `img-<hex>` is its **image handle**: content-derived, and stable for as long as the image exists. `thread:N` is its position, counted 1-based across the conversation. On upload, a fast vision model describes each one, and that description is kept as a derived fact.

- **Prefer the handle, and note it if you may want the image later.** `thread:N` renumbers whenever an earlier image turns up, so a noted number can end up naming a different picture. Every tool that takes an image reference accepts both forms.
- **Copy the handle, never count.** An image attached to the current message carries its handle in its label, e.g. `[1 image attached to this message (img-0123456789abcdef)]`. Pass that handle; do not work out its `thread:N`.
- **Recent images are already in your vision**: you can see and describe them natively, so just answer.
- **Older images age out of your vision** after a few newer messages, so stale screenshots don't mislead you in long threads. The history then shows only a text note like `[attached image (thread:2, img-0123456789abcdef, 2h ago, image not included, may be outdated)]` plus the saved description.
- **To see an aged-out image again, call `view_image`** with its handle, e.g. `view_image(image: "img-0123456789abcdef")`. It reloads the actual pixels into your vision. **When the user refers to an earlier image you cannot see, call `view_image`.** Never reply that there is no image, and never ask them to re-send it. Take the address from the history's image notes.
- **`[Image not sent: <model> does not read images. …]` means this model is text-only.** Its vision flag is off, so no image reaches it, `view_image` included. Answer from the saved description. If that is not enough, say so, and point the user at the model's vision flag in Settings → Models.
- **To keep an image** as a file, use `save_thread_image(image: "img-<hex>", path: "...")` (writes under `data/artifacts/`, git-committed). **To edit or restyle an image**, use `generate_image` with `input_images: ["img-<hex>"]`. To view an image *file* already under `data/artifacts/`, use `read_file`, not `view_image`.
