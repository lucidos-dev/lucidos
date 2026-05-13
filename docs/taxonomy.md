# Lucidos Workspace Taxonomy

Source of truth for how workspace content is organized. Referenced by both the engine system prompt (for the Lucidos LLM) and CLAUDE.md (for CC development sessions).

## Three Content Types

| Type | Purpose | Stability | Who maintains | Example |
|------|---------|-----------|--------------|---------|
| **Intent** | What the user wants, in their terms. Goals, conditions, desired outcomes. | Stable — changes when the user's needs change | User | "Find relevant jobs, store them, notify me of new ones and deadlines" |
| **Knowhow** | How to achieve it, in technical terms. API details, data formats, quirks, workarounds. | Evolves — refined every time Lucidos learns something new | Lucidos | "Use `.product-card img` selector, vendor CDN requires base64 conversion" |
| **Script** | Code invoked by intents or knowhow | Changes when tools or APIs change | Either | `download_images.py`, `validate_images.py` |

### Intent vs Knowhow

The intent describes **what the user wants** — written in user terms, like what you'd tell a competent assistant. The knowhow describes **how to do it well** — technical details that Lucidos accumulates over time.

**Test: "Would a non-technical person understand this file?"**
- Yes → it's an intent
- No → it's knowhow

Example: A product-watch intent says "find relevant listings for me, store them, notify me if there are new ones or upcoming deadlines." The knowhow explains how vendor logos are extracted, how prices are normalized, what CORS workarounds are needed. When Lucidos discovers a new quirk, it updates the knowhow — the intent stays the same.

### Frontmatter

Both intents and knowhow files use YAML frontmatter:

**Intent frontmatter:**
```yaml
---
name: Daily Weather Check
knowhow:
  - weather-api
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

When Lucidos discovers something new during execution (a quirk, a better approach, a failure mode), it should update the relevant **knowhow** file. Knowhow is Lucidos's living memory of how to do things well. Intents should only change when the user's goal itself changes — never put technical details in intents.

## Ownership Principle

**Everything lives with its consumer.** Intents, knowhow, and scripts are always scoped to the thing that uses them — an app, a trigger, or a knowhow domain.

**Survivability test:** "Does this survive if I delete the app?" If yes, it belongs at the top level (e.g., Google Calendar sync). If it only makes sense in the context of the app (e.g., a per-app scoring heuristic), it belongs inside the app.

## Directory Structure

```
data/
  artifacts/                ← User files (notes, imported data, projects) — git-tracked, NEVER auto-delete
    user_profile.md         ← Learned facts about the user
    imported/<service>/     ← Files imported from APIs (e.g., oura/, weather/)
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

- **File naming:** Never use generic names like `skill.md`, `knowhow.md`, or `intent.md`. Always name files by what they describe (e.g., `calendar-data-layout.md`, `weather-forecast.md`, `comfort-cloud-api.md`).
- **Everything under `data/` (except `postgres/`) is git-tracked** — files persist and have version history.
- **`.lucidos/`** is ephemeral (runtime cache, temp files). Can be rebuilt. Not under `data/`.
- **Manifest vs knowhow:** `manifest.json` is for the user (UI display). Knowhow and intents are for the engine (LLM context). Don't put operational knowledge in manifests.
- **Scripts belong with their consumer** — if only one trigger uses a script, it goes in that trigger's `scripts/`. If only one app uses it, it goes in that app's `scripts/`.

## Apps

Apps have two layers of metadata:

- **`manifest.json`** — user-facing: name, description, icon. Displayed in the app list UI. The LLM does NOT see this in its context.
- **`knowhow/` + `intents/`** — engine-facing: injected into LLM context when the app is active. This is how the LLM knows what the user wants and how to achieve it.

Optional `knowhow` field in manifest references general knowhow (from `data/knowhow/`) that the app also needs.

Data storage: pick artifacts (git) OR events (postgres), not both.

## Triggers

Triggers are scheduled tasks that run on cron or in response to events. The **intent** lives in the `TriggerCreated` event payload (`run.intent`) — there is no `intent.md` file. The intent is a single sentence in the user's voice (`{ type: 'intent', intent: '...' }`); there is no per-trigger knowhow allow-list to configure. When the trigger fires, the spawned thread looks up knowhow itself via `load_knowhow` — same as a chat session. Optional **scripts** and trigger-scoped **knowhow files** live alongside the trigger on disk.

### Intent vs Knowhow Split (the rule everyone gets wrong)

Triggers tempt you to dump procedure into the intent — there's one big text field, the API doesn't enforce structure, and the procedure is fresh in your head when you create it. **Resist.** Every imperative verb about *how* (hit, parse, scan, fall back, retry, emit) belongs in a knowhow file the LLM can discover, not in the intent.

- **Intent**: a sentence the user would say. "Notify me when GPT-5.5 is available via the OpenAI API."
- **Knowhow**: the recipe. "GET `/v1/models` with `Authorization: Bearer $OPENAI_API_KEY`; scan `data[].id` for ids starting with `gpt-5.5`; on 401/403/network error fall back to one `web_search` for ..."

Test: would a non-technical person understand the intent? If no, knowhow has leaked.

### Worked Example

**Bad** (intent contains the recipe):
```
run.intent: "Check whether gpt-5.5 is available. GET https://api.openai.com/v1/models
with Authorization: Bearer $OPENAI_API_KEY. Scan data[].id for any id starting
with gpt-5.5. If found, send_notification + update_trigger to disable. If 401/403
or network error, fall back to web_search for 'gpt-5.5 OpenAI API available'.
If not yet available, stay silent."
```

**Good** (recipe lives in a knowhow file the trigger thread will discover at fire time):
```
# data/knowhow/openai-api-availability.md
---
name: OpenAI API Model Availability
description: How to check whether a specific model is reachable via the OpenAI API.
---
GET `https://api.openai.com/v1/models` with `Authorization: Bearer $OPENAI_API_KEY`.
On 200, scan `data[].id` for the requested model prefix. On 401/403/network error,
fall back to one `web_search` distinguishing API availability from ChatGPT-only rollout.
```
```
run.intent: "Notify me when an OpenAI model with id prefix gpt-5.5 becomes available
via the API. Once notified, disable this trigger."
```

When OpenAI changes their endpoint, you update one knowhow file — not every trigger that touches it. When a new model needs watching, you create a new trigger; the same knowhow surfaces via semantic discovery.

### Knowhow discovery at fire time

The trigger thread inherits the chat-thread knowhow surface: the system prompt advertises the intent registry, and the LLM calls `load_knowhow` when it judges a recipe relevant. There is no allow-list to configure on the trigger and no pre-load step. Make the knowhow file's `name` and `description` frontmatter precise so semantic discovery finds it.

### Order of Operations

When creating a trigger that needs a recipe, write the knowhow file first (so it exists when the trigger fires and discovery can surface it), then create the trigger.

### Locations

- **Standalone triggers** live in `triggers/<slug>/` (intent is event-sourced; the directory holds scripts and trigger-specific knowhow). `<slug>` is the trigger's kebab-case `slug` field.
- **App-specific triggers** live in `apps/<id>/triggers/<slug>/`.
- **Trigger-scoped knowhow** lives at `data/triggers/<slug>/knowhow/<descriptive>.md` (or `data/apps/<id>/triggers/<slug>/knowhow/` for app-specific triggers). Visible only to threads of trigger `<slug>`.
- **Shared knowhow** lives in `data/knowhow/<id>.md` (or `data/knowhow/<id>/<descriptive>.md` for multi-file domains).

## Thread Vocabulary

Threads spawned by other threads come in two flavors. The `relation` argument on `run_thread` / `run_claude` (and the `--relation sub|top` flag on `lucidos spawn-thread`) chooses between them.

| Term | Meaning |
|------|---------|
| **Spawning thread** | The thread that issues the spawn (verb-derived, neutral). |
| **Spawned thread** | The new thread the spawn creates. |
| **Sub-thread** | A spawned thread with a parent. When it reaches a terminal state, the engine fires a callback that resumes the parent with the sub-thread's result. `relation: "sub"` (default for `run_thread` / `run_claude`); `--relation sub` on the CLI. |
| **Top-thread** | A spawned thread with no parent. Appears in the main thread list as an independent top-level thread. The spawning thread is NOT resumed when it finishes. `relation: "top"`; `--relation top` (CLI default). |
| **Parent thread** | The spawning thread of a sub-thread (only meaningful when callback wiring is active). |

Database columns (`parent_thread_id`, `child_thread_id`) keep their names — sub-threads still have parents. New prose should standardize on the terms above; "child thread" is an older spelling of "sub-thread" still in use in some files.
