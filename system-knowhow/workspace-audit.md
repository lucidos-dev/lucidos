---
name: Workspace Consistency Audit
description: Recipe for auditing the workspace's apps, triggers, knowhow, intents, scripts and artifacts against current Lucidos conventions and the SDK/CLI surface. Use when the user asks to audit the workspace, check for drift, or "see what's stale".
---

# Workspace Consistency Audit

A read-only sweep of a Lucidos workspace that reports drift between what's on disk and how the system currently expects things to look. Output: one Markdown report under `data/artifacts/audits/` plus a `WorkspaceAuditCompleted` event.

## When to run this

User says "audit the workspace", "check for drift", "what's stale", "is everything still using the right pattern", "scan my apps/triggers". Or after a major change to the SDK / CLI / system prompt where existing content might silently use the old shape.

**Read-only.** Never edit or delete during the audit. The report proposes fixes; the user (or a follow-up session) decides what to apply. This covers *every* mutation, not just the ones the checks below name — no `rmdir`/`rm`, no writing a `.gitignore`, no `git add`/`git commit`, no `run_coding_agent`. If you catch yourself running a mutating command mid-sweep, you have left the recipe: put the fix in the report instead. Remediation, when the user asks for it, has its own rules — see § Remediation.

## Sources of truth — load these first

This knowhow does **not** restate the rules. It points at them. Each check below names the file that owns the rule; load that file when you need the canonical wording.

| Reference | Owns |
|---|---|
| `system-knowhow/best-practices.md` | Workspace file conventions: artifacts/, apps/, knowhow/, intents/, scripts/, config/ layout, per-workspace environment variables (Settings → System → Environment variables / the grouped `env_vars` tool), naming, "never nest artifacts", import-the-minimum |
| `system-knowhow/js-sdk.md` | Current app HTML boilerplate and the full `lucidos.*` API surface (anything not listed is either deprecated or invented) |
| `system-knowhow/lucidos-cli.md` | What scripts and coding-agent subprocesses use for `data.*` writes, `events.*` emits, `proxy` calls to external APIs (preferred over raw `curl -H "Authorization: ..."` with `$CRED_*`), and `spawn-thread` thread spawning (sub-threads + cross-workspace, including Codex via `--codex`) |
| `system-knowhow/intent-registry.md` | Which on-disk files become intents in the system prompt (trigger files double as intents — easy to miss) |
| `system-knowhow/thread-events.md` | Every `ThreadEvent` name, which of them a trigger can subscribe to, and which names are **retired aliases** that still decode old rows but no longer fire a subscription |
| The active engine system prompt | The intent vs knowhow taxonomy, the trigger worked example |

When a check below says "per `<file>`", that means: read the current version of that file and use *its* wording — not your memory of what it said.

## What to walk

Resolve `data/` paths via the `lucidos` CLI. The audit covers:

| Surface | Path |
|---|---|
| Apps | `data/apps/<id>/` |
| Standalone triggers | `data/triggers/<slug>/` |
| App-scoped triggers | `data/apps/<id>/triggers/<slug>/` |
| Shared knowhow | `data/knowhow/<id>.md` and `data/knowhow/<id>/` |
| App-scoped knowhow | `data/apps/<id>/knowhow/` |
| Trigger-scoped knowhow | `data/triggers/<slug>/knowhow/` (visible only to threads of trigger `<slug>`) |
| Intents | `data/apps/<id>/intents/`, `data/apps/<id>/triggers/`, `data/triggers/<slug>/*.md` — all three feed the registry; see `system-knowhow/intent-registry.md`. There is no top-level `data/intents/` source. |
| Scripts | `data/scripts/`, `data/apps/<id>/scripts/`, `data/triggers/<slug>/scripts/`, `data/knowhow/<id>/scripts/` |
| Artifacts (structural only) | `data/artifacts/` |

Trigger intent text lives in the `TriggerCreated` event payload (`run.intent`), not on disk — pull via `lucidos events query --type TriggerCreated`.

## What to check

For each finding, capture: **location**, **what's wrong**, **which reference owns the rule** (link, don't quote), **suggested fix**.

### 1. Triggers — intent vs knowhow split

**Start from the live projection (`list_triggers` / the trigger registry the scheduler uses), not from `TriggerCreated` events.** Only audit triggers present in the live list — anything else has been deleted (`TriggerDeleted`) or is otherwise no longer firing. Use `TriggerCreated`/`TriggerUpdated` events purely to reconstruct historical `run` fields the projection doesn't expose (intent text, stale `run.knowhow:[...]`, slug) for triggers that are still live.

Walking `TriggerCreated` alone produces phantom findings — "broken" triggers that don't actually exist anymore — and wastes the user's time chasing them. The scheduler's live state is the source of truth for *what is currently scheduled*; events are the source of truth for *what the run config historically looked like*.

Per the engine prompt's taxonomy section and the worked example in `docs/taxonomy.md` (mirrored in best-practices). Trigger threads discover knowhow at fire time via `load_knowhow` (same as chat); there is no per-trigger allow-list on the trigger config. For each *live* trigger, reduce its `TriggerCreated` + subsequent `TriggerUpdated` events to the *latest* `run` (most recent payload by sequence) before applying these checks:

- **Imperative verbs about *how* in `run.intent`** (hit, parse, scan, fall back, retry, GET, POST, scrape) → procedure leaked into intent.

- **Stale `run.knowhow: [...]` field.** Per `system-knowhow/triggers.md` § "Setup checklist" item 5: legacy `run.knowhow:[...]` is silently dropped by the deserializer. The trigger keeps firing, but no knowhow gets pre-loaded, so the LLM's behavior now depends on whether it picks the same files up via discovery. Surface each affected trigger's id and the `knowhow` ids it used to request, and recommend the rewrite the source file specifies. Severity: **stale** (silently broken).

- **Subscription on a retired event name.** For each live trigger's `on` list, check every `event_type` against `system-knowhow/thread-events.md`. **A rename does not carry subscriptions with it**: the matcher compares the event type as an exact string, so a trigger naming a retired event stops firing the day the rename ships, with no error, no failed run, and nothing in the trigger's own history to notice. It reads exactly like an event that simply never happened, which is why these are not found without an audit. Classify each name:
  - **Current**: appears as a variant name in the left-hand column of one of that file's tables. Fine.
  - **Retired**: appears only *inside* a row, as its `Legacy alias:` note (e.g. `MemorySearched` is now `MemoryRecalled`, `ClaudeCodeIdled` is now `CodingAgentIdled`, `ContextAssembled` and `ContextTokensMeasured` are now `ContextCaptured`). Flag it and name the current event to re-point at. Severity: **broken** (silently stopped firing).
  - **Neither**: most likely a workspace *domain event* the workspace emits itself via `emit_event`, whose names are arbitrary by design and are deliberately absent from that file. Confirm with `lucidos events query --type <name>` before saying anything. Flag only when the workspace has never emitted one, and then as **smell** (a subscription on a name nothing produces), never as a rename.

  The third branch is the one that invents findings when skipped: the audit must not tell a user their own domain event is a misspelling.

- **Retired event names in workspace code.** Same retirement list, different surface. A stale recipe keeps minting subscriptions that can never fire, so the finding above returns after the user fixes it. Grep for retired names in `await_event` calls, `on_event` payloads, and `lucidos triggers` invocations across `data/knowhow/**/*.md` (fenced `python` / `bash` / `js` / `ts` blocks), `data/scripts/**`, `data/apps/**`, and `data/triggers/**/scripts/**`. Surface path + line + the retired name + its current name. Severity: **stale**. Do NOT rewrite during the audit: the audit stays read-only.

- **Missing explicit `slug` field.** Per `system-knowhow/triggers.md` § "Setup checklist" item 5: when slug isn't persisted on the event, the engine derives it from the *create-time* name on every read, so a renamed trigger keeps a folder named after its old name. Recommend persisting an explicit `slug` (CLI `lucidos triggers update --slug`, or the HTTP API, where the LLM tools take none) when the trigger has (or will have) per-trigger knowhow files. Severity: **nit** (preventive).

- **Per-trigger knowhow dir orphaned from any live trigger**: for each directory under `data/triggers/<slug>/knowhow/`, confirm `<slug>` matches the slug of an active (non-deleted) trigger. Knowhow under an unreferenced slug is invisible (the system prompt scopes by exact slug match); flag and recommend renaming the directory to a live slug or deleting it. If the trigger runs a **script**, moving its folder also requires `update_trigger(run.path=…)`, since the registered path does not follow the folder, and deleting the old folder before that event lands breaks the next fire (see `triggers.md` § "Renamed trigger → stale `run.path`"). Reference: `system-knowhow/triggers.md`.

- **Notification routing: `tap` opt-ins for CTA-shaped triggers.** For each trigger whose `run.intent` mentions `send_notification` (or each `NotificationCreated` event traceable to a trigger), look at the body the trigger produces. Per `system-knowhow/triggers.md` § "Notification routing":
  - Body reads like a direct CTA inside an app ("check in", "open <X>", "tap to log") **and** the trigger sets `app_id` → suggest `tap: { kind: 'navigate', to: { target: 'app', app_id: '<id>' } }` so the tap skips the modal.
  - Body reads like a question or prompt that needs the user back in the conversation ("coding agent is asking", "needs your input", "respond to") → suggest `tap: { kind: 'navigate', to: { target: 'thread', id: '<thread_id>', event_id: '<event_id?>' } }`.
  - Body reads like a multi-result panel-shaped destination ("N changes ready to apply", "3 triggers failed overnight") → suggest `tap: { kind: 'navigate', to: { target: 'changes' | 'triggers' | 'files' | 'notifications' } }` for the matching panel.
  - Body reads like a purely informational push ("backup complete", "sync finished", "5 tasks today") or a status report the user re-reads later ("daily summary", "weekly digest") → leave `tap` at the default `{ kind: 'modal' }`. Every notification is openable — there is no passive kind (the old `{ kind: 'none' }` was retired).
  Severity: **drift** (current default works, opt-in tightens UX). Skip when the trigger already sets `tap` to a non-default value.

- **Old-form `tap` strings — schema drift.** The `tap` field used to be a four-string union (`'modal' | 'open_app' | 'open_thread' | 'none'`); the current shape is a discriminated union object (`{ kind: 'modal' | 'navigate', to?: NavigateUi }`). The engine hard-rejects old-form strings with `400 Bad Request` at write time. The object form `{ kind: 'none' }` is retired too — the engine coerces it to `{ kind: 'modal' }` on write, so it won't 400, but workspace code still emitting it is stale and should be rewritten to `{ kind: 'modal' }`. Walk:
  - `data/triggers/**/scripts/*.{py,sh,js,ts}` — grep for `"tap": "modal"`, `"tap": "open_app"`, `"tap": "open_thread"`, `"tap": "none"`, the object form `{ "kind": "none" }`, and the single-quoted / JS-syntax variants.
  - `data/apps/**/{ui,**}.{js,ts,html}` — grep for `tap: 'modal'` / `tap: 'open_app'` / etc. and the double-quoted variants.
  - `data/scripts/**/*.{py,sh,js,ts}` — same patterns.
  - `data/knowhow/**/*.md` — fenced code blocks (`python`, `bash`, `js`, `ts`) carrying the same patterns; stale recipes propagate the leak.

  For each match, surface the file path + line + the matched form. Recommend running `system-knowhow/migrate-tap-shape.md` to rewrite all in place. Severity: **broken** (will 400 at next fire). Reference: `system-knowhow/js-sdk.md` § `lucidos.notifications`.

  Do NOT rewrite during the audit — the audit stays read-only. The migration recipe is the surface that actually edits files.

### 2. Apps — SDK boilerplate and structure

Per `system-knowhow/js-sdk.md`:

- `index.html` matches the current boilerplate (script order, which pieces are required vs optional).
- Every `lucidos.*` call used in app code appears in the SDK reference. Calls not listed are either deprecated or invented.
- **External-API calls from the iframe — USE `lucidos.proxy(name).fetch(path, init)`.** The engine forwards the request server-side, injects the configured auth header from the credential store, and strips Cookie/Origin/Referer/Host. The credential never reaches the iframe. Configure the backend once in `data/config/apis.json`. Reference: `system-knowhow/js-sdk.md` § `lucidos.proxy`.

  **DO NOT USE** any of the following — flag each occurrence and recommend the SDK helper:

  - `fetch('http://...')` or `fetch('https://<external-host>/...')` from inside an iframe. Mixed-content / CORS blocks it; if it works the credential is sitting in the iframe. Suggest adding a `data/config/apis.json` entry and switching to `lucidos.proxy(name).fetch(...)`.
  - `fetch('/api/v1/proxy/<name>/...')` — same wire format as the SDK helper, but bypasses it. The proxy name becomes a magic string (typo-prone, undiscoverable), and future SDK-side concerns (timeouts, retries, response parsing, error shape) won't apply. Suggest switching to `lucidos.proxy('<name>').fetch(path, init)`.
  - Hardcoded `Authorization` / `X-API-Key` / `Bearer ...` headers in app code. The credential belongs in the engine credential store, referenced by name from `apis.json`. Suggest moving the credential and switching the call to `lucidos.proxy(name).fetch(...)`.

- **Hand-built engine URLs in app JS.** Walk `data/apps/**/*.{js,ts,html}` for an `/api/v1/` path constructed in JavaScript rather than in markup: `fetch('/api/v1/…')`, `new URL('api/v1/…', document.baseURI)`, `new EventSource('/api/v1/events')`, and any `location.pathname`-splicing that rebuilds the workspace address by hand. All of them 404 behind the gateway (an app iframe has no `<base href>`, and a leading `api` is read as a workspace name), and the engine's markup rewriter cannot reach a URL built at runtime. Recommend an SDK method, or `lucidos.apiUrl(suffix)` where none covers the endpoint. Severity: **broken**, and worth flagging even when the app looks fine: the 404 usually lands in a `catch` that logs and falls back, so the symptom is stale data rather than an error. Reference: `system-knowhow/js-sdk.md` § `lucidos.apiUrl`. An `/api/v1/` path in a markup `src` / `href` attribute is correct and must NOT be flagged.

Per `system-knowhow/best-practices.md`:

- `manifest.json` carries only user-facing metadata; no operational knowledge has leaked in.
- Single-app docs live under `apps/<id>/knowhow/`; multi-consumer docs live under shared `data/knowhow/`.
- App data persists under `data/artifacts/<app-id>/`.

### 3. Knowhow — naming, frontmatter, and content

Per `docs/taxonomy.md` (frontmatter shape) and `system-knowhow/best-practices.md` (file placement):

- Frontmatter has `name` (required) and `description` (recommended — semantic discovery uses it).
- Filename is descriptive, not generic.
- App-scoped knowhow doesn't reference things outside its app; shared knowhow doesn't name specific apps.
- **Orphaned files under `data/knowhow/`**: a knowhow file whose id appears in NO trigger's stale `run.knowhow` (see § 1), in NO intent's `knowhow:` frontmatter (see § 4), and in NO app `manifest.json`/`config/*.json` reference is potentially dead. Most common cause is a trigger that lost its `run.knowhow` reference when the preload was retired and never had its content moved into the trigger's intent. Surface the file path and recommend either (a) inlining the relevant procedure into a trigger's intent, (b) moving the file into `data/triggers/<slug>/knowhow/` if it was always trigger-specific, or (c) deleting it if no consumer remains. Severity: **stale** (review). Reference: `system-knowhow/triggers.md`.

Per `system-knowhow/lucidos-cli.md` and `system-knowhow/best-practices.md` § `config/`, also grep knowhow bodies for pre-proxy API patterns — knowhow tells future LLM/coding-agent sessions *how* to call APIs, so a stale recipe propagates the leak even after apps and scripts are clean. Patterns to flag:

- `curl -H "Authorization: Bearer $CRED_<NAME>"` (or any inline credential header) in a code fence labelled `bash`/`sh`.
- `requests.get(url, headers={"Authorization": f"Bearer {os.environ['CRED_...']}"})` and equivalents in Python.
- Prose instructing the LLM to "set `$CRED_X`" or "use the credential from the environment" for an API the workspace owns a credential for.

Suggest rewriting the recipe around `proxy_request` (LLM tool), `lucidos.proxy(name).fetch(...)` (SDK, in apps), or `lucidos proxy <name> ...` (CLI, in scripts), with the backend configured once in `data/config/apis.json`.

### 4. Intents — frontmatter and tone

Scope is **every `.md` file the registry reads** (per `system-knowhow/intent-registry.md`): `apps/<id>/intents/`, `apps/<id>/triggers/`, and `triggers/<slug>/`. Trigger `.md` files are intents too — don't skip them. If an ID appears in the engine's "Available Intents" list but you can't find a file under `intents/`, look in the sibling `triggers/` directory before flagging it as a phantom.

- `name` present.
- `knowhow:` IDs in the frontmatter (if any) resolve to existing files — severity **broken**. An ID is the path under `data/knowhow/` (or `system-knowhow/`) without the `.md` suffix INCLUDING any subdirectory. The most common drift is a bare basename when the file lives in a subdirectory: `'nightly-pipeline-trigger'` for a file at `data/knowhow/lucidos-ops/nightly-pipeline-trigger.md` (correct id: `lucidos-ops/nightly-pipeline-trigger`). Resolve each id by listing `data/knowhow/` (and `system-knowhow/` for prefixed ids) and matching the full relative path.
- Reads in user terms, not engineer terms (same test as triggers).
- **Do not flag:** a missing `data/triggers/<slug>/<slug>.md` for a *standalone scheduled trigger* is not drift on its own. The trigger's `run.intent` (captured in the `TriggerCreated` payload) is sufficient for scheduled firing. An on-disk procedure file under `data/triggers/<slug>/` is only warranted when the procedure has dual use — scheduled firing **and** on-demand `execute_intent` invocation. Pure scheduled orchestrators that nothing ever calls manually are correct as-is.

### 5. Scripts — CLI usage and isolation

Per `system-knowhow/lucidos-cli.md`:

- Writes to `data/` go through `lucidos data write`, not raw HTTP and not open-coded paths under `$LUCIDOS_WORKSPACE/data/`.
- Domain events go through `lucidos events emit` / `lucidos events query`.
- External API calls go through `lucidos proxy <name>` when the workspace owns a credential for the service. Patterns to flag:
  - `curl -H "Authorization: Bearer $CRED_<NAME>"` — credential leaks into argv and shell history. Suggest configuring the backend in `data/config/apis.json` and switching the script to `lucidos proxy <name>`.
  - `curl` with the credential value pasted inline — same fix.
  - `requests.get(url, headers={"Authorization": f"Bearer {os.environ['CRED_...']}"})` and equivalents in Python — same fix.
- No hardcoded absolute paths to a specific workspace.

Per `system-knowhow/best-practices.md`:

- Script lives with its sole consumer (single-app script in `apps/<id>/scripts/`, not shared `data/scripts/`).

### 6. Artifacts — structural only

Don't enumerate user content. Per `system-knowhow/best-practices.md`:

- No `data/artifacts/artifacts/`.
- No bulk imports under `data/artifacts/imported/<service>/` that match the "dumped repo / archive" anti-pattern (file count + size are the tell). Suggest moving bulk to `.lucidos/tmp/` or `~/.lucidos/data/`.
- No orphaned `imported/<service>/` directories — flag for review (don't auto-delete).
- App data sits under `data/artifacts/<app-id>/`, not at the artifacts root.

### 7. Cross-cutting

- Broken references: missing `knowhow:` ID, missing script path, manifest pointing at a deleted asset.
- Duplicated content: same knowhow text in two files, same script copied between apps.
- Patterns the source-of-truth files explicitly mark deprecated — grep for the old form, point at the doc that flags it.

## Output

Write to `data/artifacts/audits/YYYY-MM-DD-HHMM/report.md` (user's local time; UTC if timezone unknown). Use `lucidos data write` so it lands in the workspace, not the worktree.

### Report structure

```markdown
# Workspace Audit — YYYY-MM-DD HH:MM

## Summary
- N findings across M categories
- Severity breakdown: <broken>/<drift>/<smell>
- Categories with no findings: <list>

## <Category>
### <item> — <severity>
**Location:** <path or event id>
**Issue:** <one sentence>
**Owns the rule:** `system-knowhow/<file>.md` § <section>
**Suggested fix:** <terse>
```

Severity:
- **broken** — referenced thing doesn't exist, or the app/trigger will malfunction
- **drift** — works today, uses an outdated pattern, will rot
- **smell** — convention violation, no functional impact yet

### Event

```bash
lucidos events emit WorkspaceAuditCompleted \
  --summary "Workspace audit: <N> findings (<broken>/<drift>/<smell>)" \
  --payload '{"artifact": "artifacts/audits/<dir>/report.md", "findings": <N>, "broken": <X>, "drift": <Y>, "smell": <Z>}'
```

## Remediation — only on request, and only as child threads

The sweep never fixes anything. Fixes happen in a separate step, and only when the user asks for them — either up front ("audit and fix what you can") or after reading the report ("go fix the app theming ones").

When you do spawn fix work:

- **Spawn child threads — omit `relation` (it defaults to `"child"`). Never pass `relation: "top"`.** A child thread reports back: when its session ends, this audit thread automatically resumes with the result, so you can confirm each fix landed, note what didn't, and update the report. The fix threads also nest under the audit in the thread drawer, and each one sitting on a pending change counts as an *attention descendant*, which bubbles the audit thread itself to the Current section — the user follows one row, not N. `relation: "top"` throws all of that away — the spawn records no parent *and* no spawning event, so nothing links the fix back to the audit that asked for it, and the report stays frozen at "suggested fix". The `"top"` wording ("for the user to follow themselves") does **not** cover audit remediation; the user asked for an audit, not for N loose threads.
  - Only exception: a fix targeting a *different* workspace must use `relation: "top"` — child callbacks don't cross a workspace boundary and the tool refuses the combination. Say so in the report and link the thread, since it won't report back.
- **One thread per target, spawned in parallel.** Issue the `run_coding_agent` calls in a single response; each reports back independently. Batch per app / per repo, not per finding — a thread that fixes six findings in one `index.html` beats six threads racing on the same file.
- **A child reporting back means its session ended, not that the fix is live.** Coding-agent work lands as a pending change the user applies. Report it as "proposed", never as "applied" or "live".
- **Fold the outcomes into the same report.** When the children have reported back, append a `## Remediation` section to the run's existing `report.md` (same timestamped directory) listing target, spawned thread link, and outcome per fix. Don't start a new report — a fresh sweep gets a fresh directory, a remediation pass does not.

## Out of scope

- **No edits or deletes during the sweep.** Suggested fixes only; see § Remediation for the ask-first fix path.
- **No code-style linting** — that's `cargo fmt` / `prettier`.
- **No `.lucidos/` or `data/postgres/`** (ephemeral / event store, not user content).
- **No per-file artifact enumeration** — structural rules only.
- **No codebase audit** (`crates/`, `cli/`, `scripts/`) — this audits the workspace's *use* of those surfaces, not the surfaces themselves.

## Idempotency

Each run gets its own timestamped directory. Don't overwrite previous reports — diffing them shows whether drift is being addressed. On same-minute collision, append a counter.

## Maintenance

When a referenced source-of-truth file changes (new SDK call, new convention, deprecation), this audit's checks may go stale. The reverse is also true: a check here that references a section heading or filename will break silently if the upstream renames it. See the `Maintaining the workspace audit` section in the repo's `CLAUDE.md` for the rule that governs when this file must be updated alongside changes to its sources.
