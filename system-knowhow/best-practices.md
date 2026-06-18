---
name: Workspace File Conventions
description: How files and data are organized in a Lucidos workspace
---

# Workspace File Conventions

How files and data are organized in a Lucidos workspace.

## artifacts/ — User Data & Content

### Fixed directories
| Path | Purpose |
|------|---------|
| `user_profile.md` | Learned facts about the user — maintained explicitly by the agent on the user's behalf (write confirmed facts the user shares); never auto-appended by background memory extraction |
| `imported/{service}/` | Data from APIs or local filesystem (e.g., `imported/oura/`, `imported/weather/`) |
| `projects/{name}/` | Major project folders — each has `notes.md` and related files |
| `screenshots/` | Browser screenshots (auto-named with timestamp) |
| `research/` | Research documents, deep dives, technical analysis |
| `generated/` | AI-generated images and content (default location) |

### App data storage
Apps that need persistent data store it in **`artifacts/{app-id}/`**:

| Folder | App | Contents |
|--------|-----|----------|
| `habits/` | Habit Tracker | `data.json` |
| `todo/` | Todo | `data.json` |
| `morning-dashboard/` | Morning Dashboard | `YYYY-MM-DD.json` |
| `google-docs/` | Google Docs | `state.json`, `cache.json` |

**Convention**: Use the app's ID as the folder name. The app's knowhow documents the data format.

### Generated content
- Default: `artifacts/generated/`
- Themed collections get their own folder (e.g., `artifacts/fargeleggingsark/`)
- Never put generated content in `artifacts/artifacts/`

## apps/ — App UIs & Logic

Each app: `apps/{id}/`

| File/Dir | Purpose |
|----------|---------|
| `index.html` | App UI |
| `manifest.json` | Name and description (user-facing, not loaded by engine) |
| `knowhow/` | App-specific reference docs |
| `intents/` | App-specific user intents |
| `scripts/` | App-specific helper scripts |
| `assets/` | Static files used by the app UI (images, fonts, PDFs) |

**App assets** (images, brosjyrer, icons) go in `apps/{id}/assets/`, NOT in `artifacts/`.

## knowhow/ — Shared Domain Knowledge

Reusable reference docs consumed by multiple apps/prompts:
- `knowhow/{domain}/` — e.g., `oura/`, `google-workspace/`
- Each file has a clear, descriptive name: `api-ref.md`, `lucidos-data-storage.md`, `data-format.md`

**Rule**: If a knowhow doc is only used by one app → `apps/{id}/knowhow/`. If used by 2+ consumers → `knowhow/{domain}/`.

## intents/ — User Intents

Intent definitions not tied to a single app. App-specific intents go in `apps/{id}/intents/`.

## scripts/ — Shared Scripts

Helper scripts invoked by intents, knowhow, or proxy auth handshakes — not tied to a single app:
- `scripts/{name}/run.py`
- App-specific scripts go in `apps/{id}/scripts/`

## config/ — Engine Configuration

Engine-read JSON files. Currently:

| File | Purpose |
|------|---------|
| `config/apis.json` | API proxy entries — maps a name to a `base_url` (and optional `auth` referencing a stored credential). Powers `lucidos proxy <name> ...` (CLI), `lucidos.proxy(name).fetch(...)` (SDK), and the `proxy_request` LLM tool. See `system-knowhow/lucidos-cli.md` § `lucidos proxy` for the schema and `system-knowhow/js-sdk.md` § `lucidos.proxy` for the iframe-side API. |

**This is the preferred way for scripts and apps to call external APIs.** Add an entry here once, then call the backend by name everywhere — the credential never appears in script source, args, env vars, log lines, or LLM tool transcripts. The pre-proxy pattern (`curl -H "Authorization: Bearer $CRED_..."` in scripts; `fetch` with the credential pasted into the iframe) is drift — see the workspace audit.

## Every subprocess call is a fresh process

`run_bash`, `run_bash_background`, `run_python`, and `run_python_background` each spawn a **brand-new process** (bash runs via `/bin/sh -c`). **No shell state carries over between calls** — an `export VAR=…`, a `cd somewhere`, and any shell functions you set in one call are gone by the next.

```bash
# call 1
export GWS_CONFIG_DIR=/Users/me/.config/gws-work   # set in this process only

# call 2 — fresh process, the export above never happened
gws calendar list        # GWS_CONFIG_DIR is empty → wrong/no account
```

Fixes, in order of preference:

1. **Inline the env var on the same line as the command**: `GWS_CONFIG_DIR=/Users/me/.config/gws-work gws calendar list`. For `cd`, chain in one call: `cd /some/dir && ./run.sh`.
2. **Same value across ALL calls in the workspace?** Define an **environment variable** (see the section right below) so every subprocess inherits it without any export.

Exception: `CRED_*`, `OAUTH_*_ACCESS_TOKEN`, and `LUCIDOS_WORKSPACE` **are** injected into every subprocess by the engine, so those appear in each fresh call with no export needed.

## Environment variables — Per-Workspace Config

For environment that must be the **SAME for every subprocess in this workspace**, define an **environment variable** (Settings → System → Environment variables). These are DB-backed, non-secret `NAME=value` pairs the engine injects as real env vars into **every** subprocess it spawns — `run_bash`, `run_python`, background tasks, scheduled scripts, triggers, and coding-agent (Claude Code / Codex) sessions.

- **You (the agent) can set one** with the `set_environment_variable` tool (`name`, `value`); the user can also add/edit them in Settings. Changes take effect on the **next** tool call / agent turn — **no engine restart**. (Exception: vars consumed by the engine's *own* shell-outs — e.g. `GIT_SSH_COMMAND` / `GH_CONFIG_DIR` used by the engine's Apply-time `git push` — are read into the engine process env at startup, so a *change* to those reaches the engine's own git on the next restart. Tool/agent subprocesses still see the change immediately.)
- **Non-secret only.** Values appear in logs, the event store, and tool-call payloads — that's intentional. For API keys, tokens, or passwords use a **credential** (`request_credential`) instead, which is injected as `CRED_<NAME>` and kept out of the event log.
- **Names** are uppercase letters/digits/underscores, not starting with a digit (e.g. `CLAUDE_CODE_USE_VERTEX`, `LUCIDOS_REPO`). Engine-owned names (`CRED_*`, `OAUTH_*`, `PG*`, `PATH`, internal `LUCIDOS_*` like `LUCIDOS_WORKSPACE`) are rejected, and engine-owned vars always win a collision.

The motivating case is **per-workspace identity** — a `gh` config dir, a Google `gws` config dir + project id, an SSH command — so `gh` / `git push` from agent subprocesses authenticate as the right account:

```
GH_CONFIG_DIR=/Users/me/.config/gh-work
GIT_SSH_COMMAND=ssh -i /Users/me/.ssh/id_work -o IdentitiesOnly=yes
```

Setup is **partly interactive** — you can set the variables, but the user must complete the auth handshake:

1. Pick a dedicated gh config dir and authenticate it once (user-run, opens a browser): `GH_CONFIG_DIR=<dir> gh auth login`.
2. For SSH push, make sure the key referenced by `GIT_SSH_COMMAND` is registered on that GitHub account.
3. Set `GH_CONFIG_DIR` and `GIT_SSH_COMMAND` via `set_environment_variable` (or Settings → System → Environment variables).

**Want a credential's secret under a specific env var name?** A credential can be given a custom env var name, so its secret injects as e.g. `GITHUB_TOKEN` **in addition to** the default `CRED_<NAME>` (an extra alias — the `CRED_` form still works) — useful when a CLI/SDK expects an exact variable name. Set it two ways: in the credential editor (Settings → credential editor), or up front when the agent requests the credential — `request_credential` takes an optional `env_var_name` arg that pre-fills the modal's "Env var name" field (the user can still edit or clear it before saving). The name must match `[A-Z_][A-Z0-9_]*` and can't be an engine-owned name (`CRED_*`, `OAUTH_*`, `PG*`, `PATH`, `LUCIDOS_*`). Single-value auth types only — it's ignored for `password` credentials (which split into `_USERNAME`/`_PASSWORD`).

> Note: the legacy `data/.env` file mechanism was retired in favour of this store. Any existing `data/.env` is migrated into the environment-variables store on the next engine startup and the file is removed.

## Key Rules

1. **Never nest artifacts** — `artifacts/artifacts/` is always wrong
2. **App data** → `artifacts/{app-id}/`
3. **App assets** → `apps/{id}/assets/`
4. **Imported data** → `artifacts/imported/{service}/`
5. **Generated content** → `artifacts/generated/` (or themed subfolder if a pattern emerges)
6. **Research** → `artifacts/research/`
7. **One source of truth** — don't duplicate files across locations
8. **Import the minimum** — never dump a whole repo, dataset, or archive into `artifacts/imported/` to grab one or two files from it. Artifact count is a performance axis: every additional file inflates linkify, file lists, scans, and per-render paths. Rules:
   - **Cloning a repo to inspect/run/extract from it** → clone into `.lucidos/tmp/{repo-name}/` (ephemeral, gitignored, won't bloat artifact count). Copy only the specific files the app actually needs into `artifacts/imported/{service}/`. If the user wants the full repo to persist (e.g., they plan to keep editing it), ASK first where to put it — don't decide unilaterally to dump it under `artifacts/`.
   - **Bulk datasets / archives** (Wikifonia-style — thousands of files where you only consume a few) → same rule. Inspect under `.lucidos/tmp/`, extract the entries the app uses into `artifacts/imported/{service}/`, leave the bulk archive out unless the user explicitly says "keep the whole archive available".
   - **Persistent bulk reference corpora the user wants to keep but not in the workspace** → `~/.lucidos/data/{name}/` (sibling to `~/.lucidos/knowhow/`, cross-workspace, persistent, agent-discoverable). Pin the absolute path in the relevant app's knowhow so converter scripts can find it. Use `lucidos data-store add {name} {source-dir}` to move an existing directory there.
   - **Intermediate / debug / one-shot render output** (e.g. cropping tiles, OMR debug pixmaps, scratch PNGs from a one-time analysis) does NOT belong in `artifacts/imported/` at all. Use `.lucidos/tmp/` and delete after the analysis.
   - **Inherited cruft from earlier sessions** — if you find unexplained files under `data/artifacts/imported/` that don't appear in the consuming app's source, scripts, or knowhow, verify each one (grep app + scripts + knowhow), ask the user about ambiguous cases, then `git rm` the dead files in a single commit with before/after artifact counts in the message.
