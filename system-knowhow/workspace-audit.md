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
| `system-knowhow/building-knowhow.md` | Knowhow doc vs reference: which files a root lists, and where a doc's own supporting files go |
| `system-knowhow/intent-registry.md` | Which on-disk files become intents in the system prompt (trigger files double as intents — easy to miss) |
| `system-knowhow/thread-events.md` | Every `ThreadEvent` name and which of them a trigger can subscribe to. For the **retired** set, ask the `events` tool rather than reading this file: see check 1 |
| `system-knowhow/migrate-tap-shape.md` | The old-form `tap` detection patterns, and the paths they live in. The audit detects and reports; that recipe is what rewrites |
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

- **Stale `run.knowhow: [...]` field.** Per `system-knowhow/triggers.md` § "Setup checklist" item 4: legacy `run.knowhow:[...]` is silently dropped by the deserializer. The trigger keeps firing, but no knowhow gets pre-loaded, so the LLM's behavior now depends on whether it picks the same files up via discovery. Surface each affected trigger's id and the `knowhow` ids it used to request, and recommend the rewrite the source file specifies. Severity: **stale** (silently broken).

- **Subscription on a retired event name.** For each live trigger's `on` list, classify every `event_type` with the `events` tool's `event_types` action. One call returns the three buckets the check needs.

  **A rename does not carry subscriptions with it.** The matcher compares the event type as an exact string, so a trigger naming a retired event stops firing the day the rename ships. No error, no failed run, nothing in the trigger's own history to notice. It reads exactly like an event that never happened, which is why these are not found without an audit.

  | Bucket the name is in | Verdict |
  |---|---|
  | `engine` | fine, it is live |
  | `retired` | **broken**, it silently stopped firing. Re-point it, see below |
  | `workspace` | fine, it is a domain event this workspace emits itself |
  | none of the three | **smell**, a subscription on a name nothing produces |

  The last row is where a rushed audit invents findings. Domain-event names are arbitrary by design, so read the `workspace` bucket before saying anything, and never call one a misspelling.

  **Ask the tool. Never assemble the retired set by hand.** `retired` is read from `ThreadEvent::LEGACY_TYPE_NAME_ALIASES`, a const a test holds to the names serde still accepts. A list built by eye from `thread-events.md` misses about half of it, and picks up frontend-only legacy names that were never event renames.

  **`retired` says a name is dead, not what replaced it.** It is a flat list, so the successor comes from `thread-events.md`. Search it for the old name: it sits on its successor's row, as a `Legacy alias:` note or in that row's prose. The whole `ClaudeCode*` family became `CodingAgent*`, which `system-knowhow/coding-agent-events.md` states once for all of them. When neither resolves it, report the finding without a replacement. A guessed successor is worse than none: re-pointing at it arms a second subscription that never fires.

  A new subscription on a dead name is refused at write time now, so what this check finds is rows armed before that landed.

- **Retired event names in workspace code.** Same retirement list, different surface. A stale recipe keeps minting subscriptions that can never fire, so the finding above returns after the user fixes it. Grep for retired names in `await_event` calls, `on_event` payloads, and `lucidos triggers` invocations across `data/knowhow/**/*.md` (fenced `python` / `bash` / `js` / `ts` blocks), `data/scripts/**`, `data/apps/**`, and `data/triggers/**/scripts/**`. Surface path + line + the retired name + its current name. Severity: **stale**. Do NOT rewrite during the audit: the audit stays read-only.

- **Missing explicit `slug` field.** Per `system-knowhow/triggers.md` § "Setup checklist" item 4: a slug not persisted on the event is re-derived from the *create-time* name on every read. So a renamed trigger keeps a folder named after its old name. Recommend persisting an explicit `slug` when the trigger has, or will have, per-trigger knowhow files. Use the CLI (`lucidos triggers update --slug`) or the HTTP API, since the LLM tools take none. Severity: **nit** (preventive).

- **Per-trigger knowhow dir orphaned from any live trigger**: for each directory under `data/triggers/<slug>/knowhow/`, confirm `<slug>` matches the slug of an active (non-deleted) trigger. Knowhow under an unreferenced slug is invisible (the system prompt scopes by exact slug match); flag and recommend renaming the directory to a live slug or deleting it. If the trigger runs a **script**, moving its folder also requires `update_trigger(run.path=…)`, since the registered path does not follow the folder, and deleting the old folder before that event lands breaks the next fire (see `triggers.md` § "Renamed trigger → stale `run.path`"). Reference: `system-knowhow/triggers.md`.

- **Notification routing: `tap` opt-ins for CTA-shaped triggers.** Take each trigger whose `run.intent` mentions `send_notification`, plus each `NotificationCreated` event traceable to a trigger. Read the body it produces and look that shape up in the table under `system-knowhow/triggers.md` § "Notification routing". Report each trigger whose `app_id`, `tap` or `event_id` disagrees with its row, quoting the row as the fix. Severity: **drift**, since the default works and the opt-in only tightens UX. Skip a trigger that already sets `tap` to a non-default value.

- **Old-form `tap` strings.** The field used to be a four-string union. It is a discriminated union object now, and the engine hard-rejects the strings with `400 Bad Request` at write time. Grep the forms listed in `system-knowhow/migrate-tap-shape.md` § "Detection patterns" across the paths in its § "Where to walk". That recipe owns both lists, so read them there rather than working from memory.

  Surface path, line and the matched form, and recommend running that recipe to rewrite them in place. Severity: **broken**, because the next fire 400s. The one exception is the retired `{ kind: 'none' }` object, which the engine coerces to `{ kind: 'modal' }` rather than refusing: **stale**. Canonical `Tap` type: `system-knowhow/js-sdk.md` § `lucidos.notifications`.

  Do NOT rewrite during the audit. The audit stays read-only, and the migration recipe is the surface that edits files.

### 2. Apps — SDK boilerplate and structure

Per `system-knowhow/js-sdk.md`:

- `index.html` matches the current boilerplate (script order, which pieces are required vs optional).
- Every `lucidos.*` call used in app code appears in the SDK reference. Calls not listed are either deprecated or invented.
- **External-API calls from the iframe — USE `lucidos.proxy(name).fetch(path, init)`.** The engine forwards the request server-side, injects the configured auth header from the credential store, and strips Cookie/Origin/Referer/Host. The credential never reaches the iframe. Configure the backend once in `data/config/apis.json`. Reference: `system-knowhow/js-sdk.md` § `lucidos.proxy`.

  **DO NOT USE** either of the following. Flag each occurrence and recommend the SDK helper:

  - `fetch('http://...')` or `fetch('https://<external-host>/...')` from inside an iframe. Mixed-content / CORS blocks it; if it works the credential is sitting in the iframe. Suggest adding a `data/config/apis.json` entry and switching to `lucidos.proxy(name).fetch(...)`.
  - `fetch('/api/v1/proxy/<name>/...')` — same wire format as the SDK helper, but bypasses it. The proxy name becomes a magic string (typo-prone, undiscoverable), and future SDK-side concerns (timeouts, retries, response parsing, error shape) won't apply. Suggest switching to `lucidos.proxy('<name>').fetch(path, init)`.

  A credential header written into app code is the same rule on one more surface. It belongs to check 7, not to this one.

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
- **A file below the listing depth that no doc names.** Per `system-knowhow/building-knowhow.md` § "Where the file goes" (mirrored in `docs/taxonomy.md` § "Knowhow: Docs and References"), a root lists `data/knowhow/<name>.md` and `data/knowhow/<group>/<name>.md`, and one level only under `data/apps/<id>/knowhow/` and `data/triggers/<slug>/knowhow/`. A file deeper than that is a *reference* belonging to the doc above it.

  That is a legitimate shape, so never flag the depth on its own. Flag only the reference **no doc names**: grep the sibling docs for its full id, which is the path under the root without `.md`. A named one is correct and silent.

  An unnamed one is unreachable. It sits in no routing list, and nothing tells the LLM the id exists. Recommend naming it from the doc that should own it, or moving it up to the listed depth so it routes on its own. Severity: **stale** (silently invisible). Nothing fails at runtime, so this check is how the user learns a reorg is needed.
- **Orphaned files under `data/knowhow/`**: a knowhow file whose id appears in NO trigger's stale `run.knowhow` (see § 1), in NO intent's `knowhow:` frontmatter (see § 4), and in NO app `manifest.json`/`config/*.json` reference is potentially dead. Most common cause is a trigger that lost its `run.knowhow` reference when the preload was retired and never had its content moved into the trigger's intent. Surface the file path and recommend either (a) inlining the relevant procedure into a trigger's intent, (b) moving the file into `data/triggers/<slug>/knowhow/` if it was always trigger-specific, or (c) deleting it if no consumer remains. Severity: **stale** (review). Reference: `system-knowhow/triggers.md`.

Knowhow bodies also carry pre-proxy API patterns, which check 7 covers. Run it over the fenced code blocks here as well as over apps and scripts. Knowhow is the surface where a leak *spreads*: it tells the next session how to call the API, so the same finding returns after the code is clean.

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
- External API calls go through `lucidos proxy <name>` when the workspace owns a credential for the service. The patterns are check 7's; a script adds one consequence of its own, which is that a credential in argv also lands in shell history.
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

- **A credential written into workspace code.** One rule over four surfaces, which is why it lives here and not inside checks 2, 3 and 5. Wherever the workspace calls an external API the engine holds a credential for, the credential belongs in the credential store and the backend in `data/config/apis.json`. Flag:

  - An inline auth header. `curl -H "Authorization: Bearer $CRED_<NAME>"`, a pasted literal token, an `Authorization` / `X-API-Key` / `Bearer` header built in JS, or `requests.get(url, headers={"Authorization": ...})` and its equivalents.
  - Prose telling a future session to "set `$CRED_X`", or to read the credential out of the environment.

  Walk `data/apps/**`, `data/scripts/**`, every `scripts/` under a trigger, and the fenced `bash` / `sh` / `python` / `js` / `ts` blocks in `data/knowhow/**/*.md`.

  The fix differs only by caller: `lucidos.proxy(name).fetch(...)` in an app, `lucidos proxy <name>` in a script, `proxy_request` for the LLM. Severity: **drift**, since the call still works. A pasted literal token is the exception worth calling out in its own line of the report: the file is git-tracked, so that credential needs rotating too, not just rerouting. Owns the rule: `system-knowhow/js-sdk.md` § `lucidos.proxy` and `system-knowhow/lucidos-cli.md` § `lucidos proxy`.

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
- Severity breakdown: <broken>/<stale>/<drift>/<smell>/<nit>
- Categories with no findings: <list>

## <Category>
### <item> — <severity>
**Location:** <path or event id>
**Issue:** <one sentence>
**Owns the rule:** `system-knowhow/<file>.md` § <section>
**Suggested fix:** <terse>
```

**Severity is a closed set of five.** Every finding carries exactly one, and the
five counters above sum to N. Use no other word:

| Severity | Means |
|---|---|
| **broken** | It does not do its job. The reference is dangling, or the app or trigger no longer works. |
| **stale** | It still runs, but on an outdated shape, or reaching nobody. Degraded, not dead. |
| **drift** | Works today, on a pattern the docs have moved off. It will rot. |
| **smell** | A convention violation with no functional effect yet. |
| **nit** | Preventive. Nothing is wrong; the current shape invites a future break. |

The split between the first two is whether the thing still does its job, never
whether it says so out loud. Most of what this audit finds is silent either way:
a trigger on a retired name is **broken** and never fires, while a trigger that
lost its preloaded knowhow is **stale** and fires without it. Neither errors.

### Event

```bash
lucidos events emit WorkspaceAuditCompleted \
  --summary "Workspace audit: <N> findings (<broken> broken, <stale> stale, <drift> drift, <smell> smell, <nit> nit)" \
  --payload '{"artifact": "artifacts/audits/<dir>/report.md", "findings": <N>, "broken": <A>, "stale": <B>, "drift": <C>, "smell": <D>, "nit": <E>}'
```

## Remediation — only on request, and only as child threads

The sweep never fixes anything. Fixes happen in a separate step, and only when the user asks for them — either up front ("audit and fix what you can") or after reading the report ("go fix the app theming ones").

### Ask once, and lead with the whole set

The user's answer is usually all of them or none of them, so ask a **single-select** card before any multi-select one. Batching them into a multi-select first charges one tap per batch for that answer. The 4-option cap also leaves no slot for an "all" option beside the batches.

Ask right after the report lands, with these options in this order:

| Option | Means |
|---|---|
| **Do all suggested fixes** | Every fix in the report. Say how many, across how many targets, in the description. |
| *A narrower cut* | Only when one exists and is genuinely safer or cheaper, such as the direct cleanups without the coding-agent work. Skip the slot otherwise. |
| **Let me pick which** | Follow with a multi-select card: one option per target batch, using the batching rule below. |
| **Nothing for now** | Leave the report as the record. It needs its own option: Submit stays disabled with nothing ticked, and Cancel aborts the turn rather than declining. |

The first label says *suggested* because that is what each finding's own line says. A label the report never uses makes the user hunt for the list it covers.

**Skip the card when the user already asked for fixes.** "Audit and fix what you can" is the answer, so re-asking it bounces a settled decision back at them.

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

When a referenced source-of-truth file changes (new SDK call, new convention, deprecation), this audit's checks may go stale. The reverse is also true: a check here that references a section heading or filename will break silently if the upstream renames it. See the `Maintaining workspace-audit` section in the repo's `.claude/rules/system-knowhow.md` for the rule that governs when this file must be updated alongside changes to its sources. `./scripts/check-knowhow-refs.sh` catches the mechanical half of that in `/harden`.
