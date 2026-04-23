# CognOS Workspace Taxonomy

Source of truth for how workspace content is organized. Referenced by both the engine system prompt (for the CognOS LLM) and CLAUDE.md (for CC development sessions).

## Three Content Types

| Type | Purpose | Stability | Who maintains | Example |
|------|---------|-----------|--------------|---------|
| **Intent** | What the user wants, in their terms. Goals, conditions, desired outcomes. | Stable — changes when the user's needs change | User | "Find relevant jobs, store them, notify me of new ones and deadlines" |
| **Knowhow** | How to achieve it, in technical terms. API details, data formats, quirks, workarounds. | Evolves — refined every time CognOS learns something new | CognOS | "Use `.company-logo img` selector, FINN CDN requires base64 conversion" |
| **Script** | Code invoked by intents or knowhow | Changes when tools or APIs change | Either | `download_images.py`, `validate_images.py` |

### Intent vs Knowhow

The intent describes **what the user wants** — written in user terms, like what you'd tell a competent assistant. The knowhow describes **how to do it well** — technical details that CognOS accumulates over time.

**Test: "Would a non-technical person understand this file?"**
- Yes → it's an intent
- No → it's knowhow

Example: A job search intent says "find relevant jobs for me, store them, notify me if there are new ones or upcoming deadlines." The knowhow explains how FINN.no logos are extracted, how salary is estimated, what CORS workarounds are needed. When CognOS discovers a new quirk, it updates the knowhow — the intent stays the same.

### Frontmatter

Both intents and knowhow files use YAML frontmatter:

**Intent frontmatter:**
```yaml
---
name: Daily Job Check
knowhow:
  - finn-no
---
```
- `name` (required): Human-readable name
- `knowhow` (optional): List of knowhow IDs to load when executing

**Knowhow frontmatter:**
```yaml
---
name: Panasonic Comfort Cloud
description: Controls and monitors Panasonic heatpumps via Comfort Cloud API
---
```
- `name` (required): Human-readable name
- `description` (optional): Short description for semantic discovery — the system matches user messages against this to automatically load relevant knowhow. If absent, derived from the name + first paragraph of the body.

### Continuous Learning

When CognOS discovers something new during execution (a quirk, a better approach, a failure mode), it should update the relevant **knowhow** file. Knowhow is CognOS's living memory of how to do things well. Intents should only change when the user's goal itself changes — never put technical details in intents.

## Ownership Principle

**Everything lives with its consumer.** Intents, knowhow, and scripts are always scoped to the thing that uses them — an app, a trigger, or a knowhow domain.

**Survivability test:** "Does this survive if I delete the app?" If yes, it belongs at the top level (e.g., Google Calendar sync). If it only makes sense in the context of the app (e.g., FINN job scoring), it belongs inside the app.

## Directory Structure

```
data/
  artifacts/                ← User files (notes, imported data, projects) — git-tracked, NEVER auto-delete
    user_profile.md         ← Learned facts about the user
    imported/<service>/     ← Files imported from APIs (e.g., oura/, finn-jobs/)
    projects/<name>/        ← Major project folders
    screenshots/            ← Captured screenshots

  apps/<name>/              ← App UIs — render in iframe with scoped chat
    manifest.json           ← User-facing metadata (name, description, icon) — shown in UI, NOT in LLM context
    index.html, styles.css  ← App UI files
    knowhow/                ← App-specific reference docs (evolves)
    intents/                ← App-specific user intents (stable)
    scripts/                ← App-specific helper scripts
    triggers/               ← App-specific scheduled triggers

  knowhow/                  ← General domain reference docs (API specs, data formats)
    <domain>.md             ← Simple knowhow (single file)
    <domain>/               ← Knowhow domain with sub-docs
      <descriptive>.md
      scripts/              ← Domain-specific scripts
      intents/              ← Domain-specific intents

  triggers/                 ← Standalone scheduled triggers (not app-specific)
    <name>/                 ← Each trigger gets its own directory
      <descriptive>.md      ← Trigger intent definition
      scripts/              ← Trigger-specific scripts

  postgres/                 ← Event store — gitignored
```

## Rules

- **File naming:** Never use generic names like `skill.md`, `knowhow.md`, or `intent.md`. Always name files by what they describe (e.g., `calendar-data-layout.md`, `finn-job-search.md`, `comfort-cloud-api.md`).
- **Everything under `data/` (except `postgres/`) is git-tracked** — files persist and have version history.
- **`.cognos/`** is ephemeral (runtime cache, temp files). Can be rebuilt. Not under `data/`.
- **Manifest vs knowhow:** `manifest.json` is for the user (UI display). Knowhow and intents are for the engine (LLM context). Don't put operational knowledge in manifests.
- **Scripts belong with their consumer** — if only one trigger uses a script, it goes in that trigger's `scripts/`. If only one app uses it, it goes in that app's `scripts/`.

## Apps

Apps have two layers of metadata:

- **`manifest.json`** — user-facing: name, description, icon. Displayed in the app list UI. The LLM does NOT see this in its context.
- **`knowhow/` + `intents/`** — engine-facing: injected into LLM context when the app is active. This is how the LLM knows what the user wants and how to achieve it.

Optional `knowhow` field in manifest references general knowhow (from `data/knowhow/`) that the app also needs.

Data storage: pick artifacts (git) OR events (postgres), not both.

## Triggers

Triggers are scheduled tasks that run on cron or in response to events. They contain:

- An **intent** (`.md` file) — what the user wants done when the trigger fires.
- Optional **scripts** — deterministic code invoked during execution.

**App-specific triggers** (e.g., daily FINN job check) live in `apps/<name>/triggers/`.
**Standalone triggers** (e.g., sleep reminder, Google Calendar sync) live in `triggers/<name>/`.
