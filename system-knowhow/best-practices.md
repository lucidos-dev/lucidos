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
| `user_profile.md` | Learned facts about the user (auto-maintained) |
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
