---
name: Workspace File Conventions
description: How files and data are organized in a CognOS workspace
---

# Workspace File Conventions

How files and data are organized in a CognOS workspace.

## artifacts/ — User Data & Content

### Fixed directories
| Path | Purpose |
|------|---------|
| `user_profile.md` | Learned facts about the user (auto-maintained) |
| `imported/{service}/` | Data from APIs or local filesystem (e.g., `imported/oura/`, `imported/finn-jobs/`) |
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
- Each file has a clear, descriptive name: `api-ref.md`, `cognos-data-storage.md`, `data-format.md`

**Rule**: If a knowhow doc is only used by one app → `apps/{id}/knowhow/`. If used by 2+ consumers → `knowhow/{domain}/`.

## intents/ — User Intents

Intent definitions not tied to a single app. App-specific intents go in `apps/{id}/intents/`.

## scripts/ — Shared Scripts

Scripts invoked by triggers, not tied to a single app:
- `scripts/{name}/run.py`
- App-specific scripts go in `apps/{id}/scripts/`

## Key Rules

1. **Never nest artifacts** — `artifacts/artifacts/` is always wrong
2. **App data** → `artifacts/{app-id}/`
3. **App assets** → `apps/{id}/assets/`
4. **Imported data** → `artifacts/imported/{service}/`
5. **Generated content** → `artifacts/generated/` (or themed subfolder if a pattern emerges)
6. **Research** → `artifacts/research/`
7. **One source of truth** — don't duplicate files across locations
