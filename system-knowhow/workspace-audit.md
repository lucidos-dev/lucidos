---
name: Workspace Consistency Audit
description: Audits the workspace apps, triggers, knowhow, intents, scripts and artifacts against current conventions and the SDK/CLI surface. Use to audit the workspace, check for drift, see what is stale, or migrate every app off one retired pattern.
---

# Workspace Consistency Audit

A read-only sweep of a Lucidos workspace that reports drift between what's on disk and how the system currently expects things to look. Output: one Markdown report under `data/artifacts/audits/` plus a `WorkspaceAuditCompleted` event.

## When to run this

User says "audit the workspace", "check for drift", "what's stale", "is everything still using the right pattern", "scan my apps/triggers". Or after a major change to the SDK / CLI / system prompt where existing content might silently use the old shape.

**Every pass is a full pass.** Run every check below yourself, on every run,
however recently the last one ran. A previous report is for comparing
*findings*, never for deciding what to skip: its coverage is only as good as the
checklist that produced it, and a check added since is a check that report never
ran. Two passes in one morning both missed a broken app that way. The first read
an outdated copy of this recipe, and the second skipped the category because the
first had called it clean.

So there is no delta pass. Re-walking a surface the previous run just fixed is
cheap, and it is also how you confirm the fix held.

## A targeted run: one check, then fix it everywhere

A **targeted run** is the other shape this recipe has, and the one to reach for
when the user already knows what broke. "Migrate my apps off `localStorage`",
"fix the apps that call the engine themselves", "we renamed X, sweep for it".
The checks below own the detection and § Remediation owns the rewrite, so a
targeted run is those two parts and nothing else.

It differs from a full pass in four ways:

- **Pick the check by what the user named**, and run that one. Where a check has
  sub-bullets, run the one that matches and say which.
- **Skip the inventory.** Walk only the surface that check names.
- **Go straight to § Remediation.** The user asking to migrate has already
  answered "do the fixes", so do not ask again, and do not stop at a report.
- **Claim only what you ran.** The report carries the one check and no
  `Categories with no findings` line. A later pass must not read it as coverage.

What a targeted run is NOT is a lighter audit. An unqualified "audit my
workspace" is always the full pass above, whatever ran this morning.

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
| The active engine system prompt | The intent vs knowhow taxonomy, the trigger worked example |

When a check below says "per `<file>`", that means: read the current version of that file and use *its* wording — not your memory of what it said.

**Reach every one of them, and this file, with `load_knowhow`.** Never go
looking for a copy in a repo by path. A Lucidos checkout carries gitignored
duplicates of its own `system-knowhow/`: build staging, and every abandoned
coding-agent worktree. Each is frozen at the commit that produced it, and a
`find` answers with whichever it walks into first. One audit re-read its own
recipe that way, got a five-week-old copy, and dropped findings it had already
collected. `load_knowhow` serves the live file and nothing else can.

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

## The mechanical scan: run this first, verbatim

Every pattern below is something a grep decides, not something you judge. Run
them as ONE call, before reading a single file, so coverage is a command that
either ran or did not. Working through them from memory is how a check gets
skipped, and a skipped check reads exactly like a clean one.

`run_bash` starts in the workspace root, so this needs no absolute path.

Each section prints its hit count, and the tail prints them again as one table.
That table is the **receipt**: proof the section ran, and the only honest basis
for calling a category clean.

| section | what it looks for | the check that judges it |
|---|---|---|
| `storage` | browser storage an app frame cannot reach | 2 |
| `host-realm` | the shell, read from an app frame | 2 |
| `engine-fetch` | the app calling the engine itself, `apiUrl` included | 2 |
| `relative-fetch` | the app fetching its own bundled file by relative path | 2 |
| `download-link` | a download link, which needs `sdk.js` in a frame | 2 |
| `media-capture` | the camera or the microphone, from an app frame | 2 |
| `web-share` | the OS share sheet, from an app frame | 2 |
| `url-mutation` | the frame writing its own session-history URL | 2 |
| `tap-strings` | the retired `tap` string forms | 1 |
| `removed-flags` | CLI flags and tool args that were removed | 7 |
| `cred-env` | a credential read from the environment | 7 |
| `auth-header` | an auth header built in app UI code | 7 |
| `machine-path` | a hardcoded home or machine path | 5 |

```bash
cd data || exit 1
ex="--exclude-dir=node_modules --exclude-dir=.venv --exclude-dir=__pycache__"
inc="--include=*.html --include=*.js --include=*.ts"
all="apps triggers knowhow scripts"
receipts=""

scan() {
  id="$1"; pattern="$2"; shift 2
  out=$(grep -rnE "$pattern" $ex "$@" 2>/dev/null)
  n=$(printf '%s' "$out" | grep -c . || true)
  printf '\n=== %s (%s hits) ===\n%s\n' "$id" "$n" "${out:-none}"
  receipts="$receipts| $id | $n |\n"
}

scan storage "localStorage|sessionStorage|document\.cookie|indexedDB" $inc apps

scan host-realm "window\.parent|parent\.document|window\.top|top\.location" $inc apps

scan engine-fetch "fetch\(['\"\`]/api/v1|new URL\(['\"\`]api/v1|new EventSource\(|apiUrl\(" $inc apps

scan relative-fetch "fetch\([[:space:]]*['\"\`][A-Za-z0-9_.-]+[/.]" $inc apps

scan download-link "<a [^>]*download" --include=*.html apps

scan media-capture "getUserMedia" $inc apps

scan web-share "navigator\.share" $inc apps

scan url-mutation "history\.(replaceState|pushState)|location\.(href|assign|replace)[[:space:]]*=|window\.location[[:space:]]*=" $inc apps

scan tap-strings "\"tap\"[[:space:]]*:[[:space:]]*\"(modal|none|open_app|open_thread)\"|tap:[[:space:]]*'(modal|none|open_app|open_thread)'|kind:[[:space:]]*'none'" $all

scan removed-flags "spawn-thread[^|]*--parent|run_coding_agent\([^)]*repo[[:space:]]*=" $all

scan cred-env "CRED_[A-Z0-9_]+" --exclude-dir=auth $all

scan auth-header "Authorization|X-API-Key" $inc apps

scan machine-path "/(Users|home)/[^\"'[:space:]]+" $all

printf '\n=== RECEIPTS: copy this table into the report ===\n'
printf '| section | hits |\n|---|---|\n'
printf '%b' "$receipts"
```

**A hit is evidence, not a finding.** The check that owns each pattern says what
the hit means, how bad it is, and what to recommend. Several are legitimate in
context: `apiUrl` building a `src` is correct, and a `try` around a storage call
changes the severity rather than clearing it. Judge every hit against its check
below, and report the ones that stand.

**The receipts table goes in the report, verbatim.** It is where a reader sees
that a category was examined rather than assumed. A `0` is a result and says the
category is clean. A section MISSING from the table is a category nobody looked
at, and the report says so rather than dropping the row: a reader counts an
absent row as clean, which is how a broken app survived two passes in one
morning.

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

- **Old-form `tap` strings.** The field used to be a four-string union. It is a discriminated union object now, and the engine hard-rejects the strings with `400 Bad Request` at write time. Grep every code-bearing surface in § "What to walk". That means trigger scripts, app code (`ui/`, `*.html`, inline `<script>`), shared scripts, and fenced `python` / `bash` / `js` / `ts` blocks in knowhow. Skip `data/artifacts/` and `data/postgres/`, neither of which holds code that calls the API.

  Match each form in both quote styles. Cover the key-quoted `"tap":` spelling (Python, shell, JSON bodies) and the bare `tap:` spelling (JS, TS):

  - `tap: 'modal'`
  - `tap: 'none'`
  - `tap: 'open_app'`
  - `tap: 'open_thread'`
  - `{ kind: 'none' }`, the retired object form

  Surface path, line and the matched form. Severity: **broken**, because the next fire 400s. The retired `{ kind: 'none' }` object is the one exception. The engine coerces it to `{ kind: 'modal' }` rather than refusing, so it is **stale**: the code reads as current while still spreading by copy. Canonical `Tap` type: `system-knowhow/js-sdk.md` § `lucidos.notifications`.

  URL-encoded and hash-form taps are **out of scope**. That channel belongs to the engine and the service worker, which are the source of truth for it, and no workspace file owns it.

  Do NOT rewrite during the audit. The audit stays read-only, and § Remediation carries the old-to-new mapping a fix thread needs.

### 2. Apps — SDK boilerplate and structure

Per `system-knowhow/js-sdk.md`:

- `index.html` matches the current boilerplate (script order, which pieces are required vs optional).
- Every `lucidos.*` call used in app code appears in the SDK reference. Calls not listed are either deprecated or invented.
- **External-API calls from the iframe: USE `lucidos.proxy(name).fetch(path, init)`.** The engine forwards the request server-side and injects the configured auth header from the credential store. It strips the headers belonging to this side of the hop. The credential never reaches the iframe. Configure the backend once in `data/config/apis.json`. Reference: `system-knowhow/js-sdk.md` § `lucidos.proxy`, which owns the strip list.

  **DO NOT USE** either of the following. Flag each occurrence and recommend the SDK helper:

  - `fetch('http://...')` or `fetch('https://<external-host>/...')` from inside an iframe. Mixed-content / CORS blocks it; if it works the credential is sitting in the iframe. Suggest adding a `data/config/apis.json` entry and switching to `lucidos.proxy(name).fetch(...)`.
  - `fetch('/api/v1/proxy/<name>/...')` — same wire format as the SDK helper, but bypasses it. The proxy name becomes a magic string (typo-prone, undiscoverable), and future SDK-side concerns (timeouts, retries, response parsing, error shape) won't apply. Suggest switching to `lucidos.proxy('<name>').fetch(path, init)`.

  A credential header written into app code is the same rule on one more surface. It belongs to check 7, not to this one.

- **The app calling the engine with its own `fetch`.** Walk `data/apps/**/*.{js,ts,html}` for any engine call JavaScript makes for itself: `fetch('/api/v1/events/query')`, `new URL('api/v1/events/query', document.baseURI)`, `new EventSource('/api/v1/events')`, `fetch(lucidos.apiUrl('/<suffix>'))`, and any `location.pathname`-splicing that rebuilds the workspace address by hand. Inside the host shell none of them reaches the engine: the frame's origin is opaque and CORS refuses it. Severity **broken**. WebKit reports it as `Load failed` and Chromium as a `TypeError`. The failure usually lands in a `catch` that falls back, so the symptom is stale data rather than an error.

  **`lucidos.apiUrl` is in that list deliberately, and the remedy depends on the endpoint.** Where an SDK method covers it, name that method. Where none does, the remedy is `lucidos.request('/<suffix>', init)`, which travels the bridge (ADR 0231). Recommending `apiUrl` for a call is how a working app gets moved onto a pattern that cannot run. Reference: `system-knowhow/js-sdk.md` § `lucidos.request`.

  **A route an app may not reach has no remedy, and saying so is the finding.** The engine classifies every route, and `lucidos.request` refuses the rest with a 403 in both realms. Credentials, thread contents, consent routes and platform control are denied on purpose. Report the app's call, name what it is reaching for, and leave it at that rather than inventing a way around. Reference: `system-knowhow/js-sdk.md` § "Not every endpoint is reachable".

  An `/api/v1/` path in a markup `src` / `href` attribute is correct and must NOT be flagged, and neither is `apiUrl` used to build one. Six are exempt from the gateway's device gate: `sdk.js`, `sdk-prefs.js`, `sdk-iframe.css`, `sdk-iframe-audio.js`, and anything under `fonts/` or `static/`. Any OTHER `/api/v1/` path in a tag is refused behind a gateway: the frame's pass to its own files reaches no engine route. Severity: **broken**, and the remedy is the `lucidos.*` method that covers it.

- **What an isolated app frame can no longer do.** Inside the host shell an app runs at an opaque origin, in its own renderer process. So it cannot freeze the shell, and it cannot read it. The price is that the frame's own `fetch`, `EventSource` and browser storage all fail, and the SDK carries those three over a bridge. Reference: `system-knowhow/js-sdk.md` § Setup, and *app frame* / *app bridge* in `docs/glossary.md`. Walk `data/apps/**/*.{js,ts,html}` for code that goes around it, skipping any vendored `node_modules/` tree:

  - **Browser storage touched directly**: `localStorage`, `sessionStorage`, `document.cookie`, `indexedDB`. Each throws a `SecurityError` in the frame. Report the two shapes apart, because they fail differently:
    - **broken** where the call sits outside a `try`. The throw stops the rest of the script, so the app renders nothing.
    - **stale** where a `try` / `catch` wraps it. That is the common shape, written for Safari private mode. It turns the break into an app that runs and silently stops remembering anything.
  - **A read of the host realm**: `window.parent`, `parent.document`, `window.top`, or the device id lifted out of the shell's storage. All blocked. Severity: **broken**.
  - **`<a href="<the app's own file>" download>`**, in an app whose `index.html` loads no `/api/v1/sdk.js`. A browser ignores `download` on a cross-origin link, so the click navigates the frame to the file instead. Severity: **broken**.
  - **The camera or the microphone.** `navigator.mediaDevices.getUserMedia`. Both browsers refuse media capture to an opaque origin outright, whatever the frame is granted, so no remedy exists inside a frame. Severity: **broken**. Say the app has to run in its own tab. Reference: `system-knowhow/js-sdk.md` § Setup, which lists what the frame is granted.
  - **The OS share sheet.** `navigator.share` called directly. The frame is not granted `web-share`, and iOS refuses the delegation anyway. Severity: **broken**. The remedy is `lucidos.ui.openExternal(url)`, which opens the link through the host. A hit inside a vendored copy of `sdk.js` is the SDK's own fallback, not the app's, so read the call site before reporting it.
  - **Session-history URL writes.** `history.replaceState`, `history.pushState`, or assignment to `location.href` / `window.location` / `location.assign()` / `location.replace()`. The frame is sandboxed without `allow-same-origin`, so the browser refuses any session-history URL write whose path or fragment differs from the frame's real URL. The error reads "Paths and fragments must match for a sandboxed document". Split the severity the way the failure does:
    - **broken** where the call sits outside a `try`. It throws, and if it is anywhere in the render or boot path the app renders nothing.
    - **stale** where a `try` / `catch` wraps it. The app runs and the URL simply stops reflecting state, so deep links out of the app stop working while nothing looks wrong.

    The remedy: an app must not write its own URL. Reading a fragment still works: apply `location.hash` at boot and subscribe to `hashchange`, which is how inbound deep links arrive. There is no app-side way to write the URL back. To share its current state, the app constructs the link string and copies or shows it. Reference: `system-knowhow/js-sdk.md` § Setup (which lists what the frame is granted) and § "fragment: opening at a place inside the app".
  - **A write to `/env-vars`**, through `lucidos.request` or the app's own `fetch`. The route opens `GET` only: a user env var reaches every command the agent runs, so a name the interpreter loads from would be host code execution. The read is untouched. Severity: **broken**, since the call answers 403 and the app's own settings never persist. § Remediation carries the replacement.
  - **A write of a human-only preference**, through `lucidos.preferences.set` or a `PUT /preferences`. The route stays open, but a key the Lucidos Agent may not write is refused to an app too: `command_guard`, `max_tool_calls`, `network_bind`, the judge settings and the `provider_enabled_*` switches. Severity: **broken**, since the call answers 403. § Remediation carries the answer.
  - **The app's own bundled file, fetched by a relative path.** `fetch('data/song.json')`, ``fetch(`audio/clips/${name}.json`)``, any `fetch` whose first argument is a relative path rather than an absolute URL. The frame's origin is opaque, so the browser refuses it exactly as it refuses an engine call. WebKit words that refusal `Load failed` and Chromium raises a `TypeError`. The path being one of the app's own files is what makes it read as safe, and the engine-fetch pattern above cannot see it: nothing in the string says `/api/v1`. Split the severity the way the failure does:
    - **broken** where the throw escapes setup, or the fetch is the app's only data path. The app renders an error banner, or nothing.
    - **stale** where a `catch` falls back to `lucidos.data` and the app keeps running, minus whatever the bundled file carried.

    The remedy is `lucidos.data.read('apps/<app-id>/<path>')`, which travels the bridge. **`lucidos.data.url()` is not a remedy.** It builds a URL for a `src` or an `href`, and fetching one is refused identically.
  - **A `<base href>` the app declares itself.** The first base in a document wins. So it replaces the pass the engine stamps for the app's own files, and behind a gateway every relative `src` / `href` then answers **401**. Severity: **broken**. The remedy is to delete it: relative refs already resolve against the app's own directory. Reference: `system-knowhow/js-sdk.md` § Setup, and [ADR 0238](https://github.com/lucidos-dev/lucidos/blob/main/docs/adr/0238-app-frame-carries-a-capability-to-its-own-files.md).

  **A separate `app.js`, `style.css` or image is NOT a finding.** It was one while the frame had no way to prove itself. The engine now gives each framed document a short-lived pass to its own files. Do not flag one, and do not recommend inlining.

  An app opened in its own browser tab is a top-level document and keeps all of this. Never report one as unaffected on that basis: the same app is reachable both ways, and the frame is the usual one.

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
- **Removed CLI flags and tool args still passed by workspace code.** Some
  shortcuts were removed after a deprecation window. A recipe still passing one
  now fails with a rename error instead of running. Grep each removed form across
  `data/knowhow/**/*.md` fenced code blocks, `data/scripts/**`, each trigger's
  `scripts/`, and `data/apps/**`, and recommend the replacement. Severity:
  **broken** (the call errors on next use). Currently removed:
  - `lucidos spawn-thread --parent` → `--relation child` (a same-workspace
    parent-with-callback spawn). Do NOT flag `threads list --parent <uuid>` or
    `threads count --parent <uuid>`, which are current, unrelated filters.
  - `repo` passed to the `run_coding_agent` tool → `folder` (which also accepts a
    registered repo name). Do NOT flag the current `lucidos spawn-thread --repo`
    flag, which is a different, live argument.

## Output

Write to `data/artifacts/audits/YYYY-MM-DD-HHMM/report.md` (user's local time; UTC if timezone unknown). Use `lucidos data write` so it lands in the workspace, not the worktree.

### Report structure

```markdown
# Workspace Audit — YYYY-MM-DD HH:MM

## Summary
- N findings across M categories
- Severity breakdown: <broken>/<stale>/<drift>/<smell>/<nit>
- Categories with no findings: <list>
- Targeted run only, in place of the line above: "Scope: <the one check>. Every
  other category is unexamined."

## Scan receipts
<the RECEIPTS table the scan printed, verbatim>

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

### Rewriting an app for an isolated frame

The storage findings need a decision rather than a rewrite. Hand a fix thread
this table and the two rules under it:

| Old | New |
|---|---|
| `localStorage.getItem('k')` holding app state | `await lucidos.data.read('artifacts/<app-id>/state.json')`, then parse |
| `localStorage.setItem('k', v)` | `await lucidos.data.write('artifacts/<app-id>/state.json', JSON.stringify(state))` |
| `localStorage.getItem('lucidos-device-id')` | delete it. The host stamps the device on every bridged call, and the frame is not meant to know which one |
| an engine call the app's own `fetch` makes | the `lucidos.*` method that covers it, else `lucidos.request('/<suffix>', init)` |
| `fetch('<the app's own bundled file>')` | `await lucidos.data.read('apps/<app-id>/<file>')`, then parse. Not `lucidos.data.url()`, which builds a `src` and is refused when fetched |
| `lucidos.request('/env-vars', { method: 'POST' })` storing the app's own setting | `lucidos.preferences` for a user-facing one, else `lucidos.data.write('artifacts/<app-id>/settings.json', …)`. The read stays, so a genuine read of the workspace's variables is left alone |
| `lucidos.preferences.set` on a human-only key such as `command_guard` | nothing. It is a security setting, and the user changes it in Settings |
| a call to a route the engine keeps from apps | nothing. Report it and name what it reaches for |
| `<a href="report.pdf" download>` on the app's own file | load `/api/v1/sdk.js`, which rewrites the click, or build a `blob:` URL |
| a `<base href>` the app declares | delete it. It replaces the pass the engine stamps for the app's own files |
| `history.replaceState(...)` / `history.pushState(...)` writing the app's own state into the URL | delete the write. Keep the read: apply `location.hash` at boot and on `hashchange` |
| `location.href = ...` / `location.assign(...)` navigating the frame itself | `lucidos.ui.navigate(...)` for a Lucidos destination, `lucidos.ui.openExternal(url)` for anything outside |

- **The read becomes asynchronous.** A synchronous `localStorage.getItem` at
  module top level becomes an `await`, so the first paint has to tolerate not
  knowing the value yet. Rewriting the call and leaving the render is how an app
  ends up reading `undefined`.
- **The value becomes workspace-wide.** `lucidos.data` is one store for every
  device, and there is no per-device app store. Some values are genuinely
  per-device: a sound toggle, a display currency. Say so in the report and let
  the user decide, rather than quietly making one shared.
- **Some findings have no fix yet, and saying so is the deliverable.** There is
  no `lucidos.storage`. Where a storage finding lands, leave the code alone and
  record it against `app-frame-escape-hatches` in `docs/temporary-measures.md`.
  A remedy written ahead of that decision moves a broken app onto a second
  broken pattern. The endpoint half of that entry is closed: `lucidos.request`
  covers an uncovered route, and a route the engine denies has no remedy by
  design.

### Rewriting an old-form `tap`

A fix thread needs the mapping, not just the finding, so hand it this table:

| Old | New |
|---|---|
| `tap: 'modal'` | `tap: { kind: 'modal' }` |
| `tap: 'none'` | `tap: { kind: 'modal' }`, since the passive kind was retired and every notification is openable |
| `tap: 'open_app'`, with a sibling `app_id: 'X'` | `tap: { kind: 'navigate', to: { target: 'app', app_id: 'X' } }` |
| `tap: 'open_thread'`, with a sibling `thread_id: 'T'` and optional `event_id: 'E'` | `tap: { kind: 'navigate', to: { target: 'thread', id: 'T', event_id: 'E' } }` |
| `{ kind: 'none' }` | `{ kind: 'modal' }` |

Three rules the rewrite has to follow:

- **Build the new sub-fields from the call's own sibling fields, and keep those siblings.** They are notification-level context. The §4 in-app matrix and the inbox modal both read them, even when the tap navigates elsewhere.
- **Reuse the expression when a sibling is computed.** Hoist a `resolve_app()` call into a local, then pass that local to both `app_id` and `to.app_id`. Calling it twice risks two different answers.
- **Flag a missing sibling, never guess one.** The old form let the engine fill the id in at write time, and the new one does not. A navigate with no thread id now raises the error toast `Navigation target missing thread id` on tap, where the old form silently did nothing. Leave a `MIGRATION REVIEW` comment naming the two ways out: supply `to.id`, or fall back to `{ kind: 'modal' }`. List every such site in the report's `## Remediation` section so the user can audit them.

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

The old-form `tap` check owns its detection patterns outright, rather than citing another file. So a change to the `Tap` type has to update them here, and the § Remediation mapping with them, in the same commit. The rule file above carries that as a row.

**§ The mechanical scan is where a pattern RUNS, and a check is where it means
something.** Some checks also name their form in words, because the report and
the remediation need it: the five `tap` strings, the two removed CLI flags.
Those are the same forms the block greps. Change one and change both, in the
same edit. A new check with a grep-able pattern needs a section in the block, or
it is a check a pass can skip without noticing.

The isolated-frame check owns its patterns the same way. A change to the app
frame's sandbox, or to what the app bridge carries, has to reach this file: what
an app can no longer do for itself is the whole of that check. That check's
`fetch` half now runs as two scan sections: `engine-fetch` for the engine
address and `relative-fetch` for the app's own files, because one CORS refusal
has two spellings in app code. A sandbox change reaches both.

When a deprecated CLI flag or tool arg is fully removed, add it to check 7's "Removed CLI flags and tool args" list. Include its replacement and any live same-named flag to exclude. The source is `docs/temporary-measures.md` § sunset deprecations.
