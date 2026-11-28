---
name: Workspace File Conventions, artifacts/: User Data & Content
description: Part of Workspace File Conventions. Its index lists the sections it covers.
---

One part of `system-knowhow/best-practices`, which lists every part.

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

Use `--font-mono` for code and `--font-ui` for the user's UI font when the theme provides them. Standalone mockups without theme variables use the native system stack: `font-family: ui-monospace, SFMono-Regular, 'SF Mono', Menlo, 'Cascadia Mono', Consolas, 'Ubuntu Mono', 'DejaVu Sans Mono', 'Liberation Mono', 'Noto Sans Mono', monospace`. Do not name user-installed coding fonts in that fallback or bundle a font for it.

### What a standalone HTML document can do

An HTML file under `artifacts/` is untrusted content: it may come from an upload or a fetched web page. So the preview runs it in a sandboxed frame at an opaque origin. The engine serves it the same way when it is opened on its own. Write for that:

| Works | Does not work |
|---|---|
| Its own `<script>`: sortable tables, charts, tabs | Calling the engine: `fetch('/api/v1/…')`, `EventSource`. The engine refuses the request. |
| Relative assets next to it, loaded as tags: `<img src="img/chart.png">`, `<link href="style.css">` | Reading a sibling file from script: `fetch('data.json')`. It is another origin, and CORS refuses it. |
| Links a reader clicks: a sibling file, `#section`, `thread:…`, `app:…`, `repo:…:file:…` | A link the script follows on its own. Only a real click navigates. |
| `<a href="results.csv" download>` in a browser, a Copy button, a video's fullscreen button | `localStorage`, `sessionStorage`, `IndexedDB`, cookies. An opaque origin has none. |
| `mailto:` and `tel:` links | Reading or changing the Lucidos window around it (`parent.…`) |

On a phone, a sideways drag anywhere in the preview swipes between Lucidos panes, as it does over an app. A page wider than the screen keeps its sideways drag, so it pans instead. A carousel or a canvas keeps its own drag by claiming it, the same way an app does (`system-knowhow/js-sdk/remote-workspace-client-contract.md` § Pane swipe).

A document that needs live workspace data, or must remember state between opens, is an **app**, not an artifact. An app reads the workspace through `lucidos.*` (`system-knowhow/js-sdk.md`), and remembers per-device state in `lucidos.storage`. If a snapshot is enough, write the data into the report, as a JSON `<script>` block or inline in the markup.

Relative assets load from `artifacts/` only. An HTML file elsewhere (`knowhow/`, an app folder) still renders, but behind the gateway its relative images and stylesheets do not load. Keep a report and its assets together under `artifacts/`.

## apps/: App UIs & Logic

Each app: `apps/{id}/`

| File/Dir | Purpose |
|----------|---------|
| `index.html` | App UI |
| `manifest.json` | Name, description, `icon` (a path to an image in the app folder; see `system-knowhow/building-an-app/scaffolding-defaults.md` § App icon), and `reveal` (when the loading cover lifts; see `system-knowhow/js-sdk/lucidos-ui-ui-control.md` § Showing the app once its content is ready). User-facing, not loaded into the LLM |
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
| `config/apis.json` | API proxy entries: maps a name to a `base_url` (and optional `auth` referencing a stored credential, and an optional `timeout_secs`). Powers `lucidos proxy <name> ...` (CLI), `lucidos.proxy(name).fetch(...)` (SDK), and the `proxy_request` LLM tool. The builtin provider proxies (`openai`, `anthropic` and the other model providers) need no entry here, and an entry with the same name overrides one. See `system-knowhow/lucidos-cli/subcommands-13.md` § `lucidos proxy` for the schema and `system-knowhow/js-sdk/lucidos-media.md` § `lucidos.proxy` for the iframe-side API. |

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
- **A name built at run time** needs a header comment: `# lucidos-credentials: CRED_A CRED_B`, or `//` in JavaScript. Its names inject like any other mention.
- **`OAUTH_<PROVIDER>_ACCESS_TOKEN`** follows the same rule. Only a named account is refreshed.
- **To see the names**, run `lucidos credentials list`. `env | grep CRED_` finds nothing, because a bare `CRED_` names no credential. The result carries a note listing the names instead.
- **An unknown name still runs.** When the text looks it up, the result carries the same note, with names only, never values. A lookup is a header comment, `$CRED_X`, `process.env.CRED_X`, `printenv CRED_X`, or `"CRED_X"` in quotes. A header comment counts only when the line starts with it. A name in prose, a header quoted in prose, or a prefix that builds a name (`CRED_<NAME>`, `f"OAUTH_{p}"`) gets no note. A scheduled script's note goes to the engine log, not to its notification.

Every subprocess output is masked: an injected value, a token in the engine's own environment, and well-known token shapes such as `ghp_…` come back as `[REDACTED]`. Output read back later, through `bash_output`, `events(action="query")` or `lucidos events query`, is masked again against the credentials stored now. A token in the shell that started the engine never reaches a chat tool's environment. To use one there, store it as a credential.

## Listing this machine's workspaces

Run `lucidos workspaces list` to see the workspaces registered with the gateway.
Each tab-separated row gives the slug, name, port, health, autostart, origin and
resolved directory. With `--json`, read the directory from `path`.
Health is `healthy`, `booting`, `restoring` or `unhealthy`; a stopped workspace
reads `unhealthy`. To stop, start or create one, see `system-knowhow/lucidos-cli/waiting-through-an-engine-restart.md` § "The
broker verbs".

It asks this workspace's engine, which asks the gateway, so it works inside the
agent sandbox. A ports file records an assignment and proves no running
process, so never infer health from one.

## The workspace database: `LUCIDOS_DATABASE_URL`

A script, trigger or agent shell reaches the workspace's own database through one variable, `LUCIDOS_DATABASE_URL`. Pass it explicitly:

```bash
psql "$LUCIDOS_DATABASE_URL" -At -c "SELECT count(*) FROM thread_summaries"
```

```python
import os, psycopg
conn = psycopg.connect(os.environ["LUCIDOS_DATABASE_URL"])
```

- **It reads, and it does not write.** Every table is readable. No table is writable, with or without `BEGIN READ WRITE`: a write fails with `permission denied for table <name>`. A direct write emitted no event, so every open page kept showing the old data. Each change has a command that announces it:

  | To change | Run | It emits |
  |---|---|---|
  | `preferences` | `lucidos preferences set --key K --value V`, or `reset --key K` | `PreferencesChanged` |
  | `notification_reads` | `lucidos notifications read --id <uuid>`, or `read-all` | `NotificationRead`, `NotificationsAllRead` |
  | `pinned_apps` | `lucidos pinned-apps pin` or `unpin`, with `--app-id` and `--device-id` | `PinnedAppPinned`, `PinnedAppUnpinned` |
  | `memory_entries` | `lucidos memory correct-by-id --id <uuid>` | `MemoryCorrected` |
  | `browser_logins` | `lucidos browser forget-login --domain D` | none |
  | Any other table | Its own `lucidos` command or API route | that route's event |

  `summary_tree_nodes` and `summary_tree_scopes` have no command. The engine rebuilds both from events, so an edit never lasted.
- **A machine-made workspace is the exception.** A workspace an agent, a demo, an eval or an e2e run created may write most tables inside `BEGIN READ WRITE; …; COMMIT;`. Secret-bearing tables, `_sqlx_migrations`, `backup_recipient`, `diagnostic_bundles`, `email_accounts` and `snapshot_restore_boundary` stay read-only there too. The gateway's registered origin decides this at boot, and an absent or unknown origin writes nothing. Such a write still emits no event.
- **It cannot read stored secrets.** It signs in as an agent role with no access to secret columns, such as `credentials.auth_value` or OAuth tokens. A query naming one fails with `permission denied`. Reach a secret through its `CRED_*` variable or the proxy instead. Every other column reads as usual. How secrets are stored: `system-knowhow/stored-secrets`.
- **Never put the URL or a password on the command line.** The command is recorded in the tool call. Naming the variable keeps the password out, and tool output masks the URL's value.
- **Bare `psql` reaches nothing.** The engine sets no `PGHOST` / `PGUSER` / `PGDATABASE` defaults and passes on no `DATABASE_URL` of its own. So an unset variable, as in `psql "$STAGING_URL"`, fails instead of landing on the workspace database.
- **A project's own database is yours to configure.** Set its `DATABASE_URL` or `PG*` names as environment variables (next section); the engine no longer reserves them.

