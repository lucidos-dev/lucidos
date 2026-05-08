---
name: Workspace Consistency Audit
description: Recipe for auditing the workspace's apps, triggers, knowhow, intents, scripts, and artifacts against the latest Lucidos conventions and SDK/CLI surface. Use when the user asks to audit the workspace, check for drift, verify their setup follows current conventions, or "see what's stale".
---

# Workspace Consistency Audit

A read-only sweep of a Lucidos workspace that reports drift between what's on disk and how the system currently expects things to look. Output: one Markdown report under `data/artifacts/audits/` plus a `WorkspaceAuditCompleted` event.

## When to run this

User says "audit the workspace", "check for drift", "what's stale", "is everything still using the right pattern", "scan my apps/triggers". Or after a major change to the SDK / CLI / system prompt where existing content might silently use the old shape.

**Read-only.** Never edit or delete during the audit. The report proposes fixes; the user (or a follow-up session) decides what to apply.

## Sources of truth — load these first

This knowhow does **not** restate the rules. It points at them. Each check below names the file that owns the rule; load that file when you need the canonical wording.

| Reference | Owns |
|---|---|
| `system-knowhow/best-practices.md` | Workspace file conventions: artifacts/, apps/, knowhow/, intents/, scripts/, config/ layout, naming, "never nest artifacts", import-the-minimum |
| `system-knowhow/js-sdk.md` | Current app HTML boilerplate and the full `lucidos.*` API surface (anything not listed is either deprecated or invented) |
| `system-knowhow/lucidos-cli.md` | What scripts and CC subprocesses use for `data.*` writes, `events.*` emits, `proxy` calls to external APIs (preferred over raw `curl -H "Authorization: ..."` with `$CRED_*`), and `spawn-thread` thread spawning (sidequests + cross-workspace) |
| `system-knowhow/intent-registry.md` | Which on-disk files become intents in the system prompt (trigger files double as intents — easy to miss) |
| The active engine system prompt | The intent vs knowhow taxonomy, the trigger worked example |

When a check below says "per `<file>`", that means: read the current version of that file and use *its* wording — not your memory of what it said.

## What to walk

Resolve `data/` paths via the `lucidos` CLI. The audit covers:

| Surface | Path |
|---|---|
| Apps | `data/apps/<id>/` |
| Standalone triggers | `data/triggers/<name>/` |
| App-scoped triggers | `data/apps/<id>/triggers/<name>/` |
| Shared knowhow | `data/knowhow/<id>.md` and `data/knowhow/<id>/` |
| App-scoped knowhow | `data/apps/<id>/knowhow/` |
| Intents | `data/apps/<id>/intents/`, `data/apps/<id>/triggers/`, `data/triggers/<name>/*.md` — all three feed the registry; see `system-knowhow/intent-registry.md`. There is no top-level `data/intents/` source. |
| Scripts | `data/scripts/`, `data/apps/<id>/scripts/`, `data/triggers/<name>/scripts/`, `data/knowhow/<id>/scripts/` |
| Artifacts (structural only) | `data/artifacts/` |

Trigger intent text lives in the `TriggerCreated` event payload (`run.intent`), not on disk — pull via `lucidos events query --type TriggerCreated`.

## What to check

For each finding, capture: **location**, **what's wrong**, **which reference owns the rule** (link, don't quote), **suggested fix**.

### 1. Triggers — intent vs knowhow split

Per the engine prompt's taxonomy section and the worked example in `docs/taxonomy.md` (mirrored in best-practices). For each `TriggerCreated` (and any subsequent `TriggerUpdated` that mutated `run`):

- Imperative verbs about *how* in `run.intent` (hit, parse, scan, fall back, retry, GET, POST, scrape) → procedure leaked into intent.
- Empty `run.knowhow` when the intent obviously needs domain knowledge (mentions an API, a service, a data format).
- **Each ID in `run.knowhow` resolves to an existing file** — severity **broken**. An ID is the path under `data/knowhow/` (or `system-knowhow/`) without the `.md` suffix INCLUDING any subdirectory. The most common drift is a bare basename when the file lives in a subdirectory: `'nightly-pipeline-trigger'` referenced for a file at `data/knowhow/lucidos-ops/nightly-pipeline-trigger.md` (correct id: `lucidos-ops/nightly-pipeline-trigger`). The engine now rejects unknown ids on `create_trigger`/`update_trigger` and aborts a pre-existing trigger's fire with a failure notification, but a paused or low-cadence trigger may not have surfaced yet — the audit should still flag it. Resolve each id by listing `data/knowhow/` (and `system-knowhow/` for prefixed ids) and matching the full relative path.

### 2. Apps — SDK boilerplate and structure

Per `system-knowhow/js-sdk.md`:

- `index.html` matches the current boilerplate (script order, which pieces are required vs optional).
- Every `lucidos.*` call used in app code appears in the SDK reference. Calls not listed are either deprecated or invented.
- External-API calls from the iframe go through `lucidos.proxy(name).fetch(...)`. Patterns to flag:
  - `fetch('http://...')` or `fetch('https://<external-host>/...')` from inside an iframe — mixed-content / CORS will block it; the credential (if any) is in the iframe. Suggest adding a `data/config/apis.json` entry and switching to `lucidos.proxy`.
  - Hardcoded `Authorization` / `X-API-Key` / `Bearer ...` headers in app code — the credential belongs in the engine credential store, referenced by name from `apis.json`.

Per `system-knowhow/best-practices.md`:

- `manifest.json` carries only user-facing metadata; no operational knowledge has leaked in.
- Single-app docs live under `apps/<id>/knowhow/`; multi-consumer docs live under shared `data/knowhow/`.
- App data persists under `data/artifacts/<app-id>/`.

### 3. Knowhow — naming, frontmatter, and content

Per `docs/taxonomy.md` (frontmatter shape) and `system-knowhow/best-practices.md` (file placement):

- Frontmatter has `name` (required) and `description` (recommended — semantic discovery uses it).
- Filename is descriptive, not generic.
- App-scoped knowhow doesn't reference things outside its app; shared knowhow doesn't name specific apps.

Per `system-knowhow/lucidos-cli.md` and `system-knowhow/best-practices.md` § `config/`, also grep knowhow bodies for pre-proxy API patterns — knowhow tells future LLM/CC sessions *how* to call APIs, so a stale recipe propagates the leak even after apps and scripts are clean. Patterns to flag:

- `curl -H "Authorization: Bearer $CRED_<NAME>"` (or any inline credential header) in a code fence labelled `bash`/`sh`.
- `requests.get(url, headers={"Authorization": f"Bearer {os.environ['CRED_...']}"})` and equivalents in Python.
- Prose instructing the LLM to "set `$CRED_X`" or "use the credential from the environment" for an API the workspace owns a credential for.

Suggest rewriting the recipe around `proxy_request` (LLM tool), `lucidos.proxy(name).fetch(...)` (SDK, in apps), or `lucidos proxy <name> ...` (CLI, in scripts), with the backend configured once in `data/config/apis.json`.

### 4. Intents — frontmatter and tone

Scope is **every `.md` file the registry reads** (per `system-knowhow/intent-registry.md`): `apps/<id>/intents/`, `apps/<id>/triggers/`, and `triggers/<name>/`. Trigger `.md` files are intents too — don't skip them. If an ID appears in the engine's "Available Intents" list but you can't find a file under `intents/`, look in the sibling `triggers/` directory before flagging it as a phantom.

- `name` present.
- `knowhow:` IDs (if any) resolve — same severity and resolution rule as triggers § 1 (full path including subdirectories; bare basenames for files in a subdirectory are the common bug).
- Reads in user terms, not engineer terms (same test as triggers).
- **Do not flag:** a missing `data/triggers/<name>/<name>.md` for a *standalone scheduled trigger* is not drift on its own. The trigger's `run.intent` (captured in the `TriggerCreated` payload) is sufficient for scheduled firing. An on-disk procedure file under `data/triggers/<name>/` is only warranted when the procedure has dual use — scheduled firing **and** on-demand `execute_intent` invocation. Pure scheduled orchestrators that nothing ever calls manually are correct as-is.

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

## Out of scope

- **No edits or deletes.** Suggested fixes only.
- **No code-style linting** — that's `cargo fmt` / `prettier`.
- **No `.lucidos/` or `data/postgres/`** (ephemeral / event store, not user content).
- **No per-file artifact enumeration** — structural rules only.
- **No codebase audit** (`crates/`, `cli/`, `scripts/`) — this audits the workspace's *use* of those surfaces, not the surfaces themselves.

## Idempotency

Each run gets its own timestamped directory. Don't overwrite previous reports — diffing them shows whether drift is being addressed. On same-minute collision, append a counter.

## Maintenance

When a referenced source-of-truth file changes (new SDK call, new convention, deprecation), this audit's checks may go stale. The reverse is also true: a check here that references a section heading or filename will break silently if the upstream renames it. See the `Maintaining the workspace audit` section in the repo's `CLAUDE.md` for the rule that governs when this file must be updated alongside changes to its sources.
