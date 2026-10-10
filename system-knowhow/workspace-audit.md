---
name: Workspace Consistency Audit
description: Audits the workspace apps, triggers, knowhow, intents, scripts, plugins and artifacts against current conventions and the SDK/CLI surface. Use to audit the workspace, check for drift, see what is stale, or move every app off a retired pattern or onto SDK UI helpers.
---

# Workspace Consistency Audit

A read-only sweep that reports drift between what's on disk and current conventions. Output: one Markdown report under `data/artifacts/audits/` plus a `WorkspaceAuditCompleted` event.

## When to run this

User says "audit the workspace", "check for drift", "what's stale", "is everything still using the right pattern", "scan my apps/triggers". Or after a major SDK / CLI / system prompt change, where existing content might silently use the old shape.

**Every pass is a full pass, with no delta pass.** Run every check yourself,
however recently the last one ran. Use a previous report to compare *findings*,
never to decide what to skip: a check added since is one it never ran.
Re-walking a surface the last run fixed is cheap, and confirms the fix held.

## A targeted run: one check, then fix it everywhere

Reach for a **targeted run** when the user already knows what broke: "migrate my
apps off `localStorage`", "fix the apps that call the engine themselves", "we
renamed X, sweep for it". It is one check plus its § Remediation, nothing else.

It differs from a full pass in four ways:

- **Pick the check by what the user named**, and run that one. Where a check has
  sub-bullets, run the one that matches and say which.
- **Skip the inventory.** Walk only the surface that check names.
- **Go straight to § Remediation.** The user asking to migrate has already
  answered "do the fixes", so do not ask again, and do not stop at a report.
- **Claim only what you ran.** The report carries the one check and no
  `Categories with no findings` line. A later pass must not read it as coverage.

A targeted run is NOT a lighter audit. An unqualified "audit my workspace" is
always the full pass.

**Read-only.** Never edit or delete during the audit; the report proposes fixes.
This covers *every* mutation: no `rmdir`/`rm`, no writing a `.gitignore`, no
`git add`/`git commit`, no `run_coding_agent`. If you catch yourself mutating
mid-sweep, put the fix in the report instead. Fixes follow § Remediation.

## Sources of truth: load these first

This knowhow does **not** restate the rules. Each check names the file that owns
its rule; load it for the canonical wording.

| Reference | Owns |
|---|---|
| `system-knowhow/best-practices.md` | Workspace file conventions: artifacts/, apps/, knowhow/, intents/, scripts/, config/ layout, per-workspace environment variables (Settings → System → Environment variables / the grouped `env_vars` tool), naming, "never nest artifacts", import-the-minimum |
| `system-knowhow/js-sdk.md` | Current app HTML boilerplate and the full `lucidos.*` API surface (anything not listed is either deprecated or invented) |
| `system-knowhow/lucidos-cli.md` | What scripts and coding-agent subprocesses use for `data.*` writes, `events.*` emits, `proxy` calls to external APIs (preferred over raw `curl -H "Authorization: ..."` with `$CRED_*`), and `spawn-thread` thread spawning (sub-threads + cross-workspace, including Codex via `--codex`) |
| `system-knowhow/plugins.md` | The plugin manifest schema, the `engine` requirement, and `check_plugin_updates` / `update_plugin` semantics |
| `system-knowhow/building-knowhow.md` | Knowhow doc vs reference: which files a root lists, and where a doc's own supporting files go |
| `system-knowhow/intent-registry.md` | Which on-disk files become intents in the system prompt (trigger files double as intents, which is easy to miss) |
| `system-knowhow/thread-events.md` | Every `ThreadEvent` name and which of them a trigger can subscribe to. For the **retired** set, ask the `events` tool rather than reading this file: see check 1 |
| The active engine system prompt | The intent vs knowhow taxonomy, the trigger worked example |

"Per `<file>`" means: read the current version of that file and use *its*
wording, not your memory of it.

**Reach every one of them, and this file, with `load_knowhow`.** Never look for
a copy in a repo by path. A Lucidos checkout carries gitignored, frozen copies of
`system-knowhow/` (build staging, abandoned coding-agent worktrees), and `find`
returns whichever it meets first. `load_knowhow` serves the live file.

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
| Intents | `data/apps/<id>/intents/`, `data/apps/<id>/triggers/`, `data/triggers/<slug>/*.md`. All three feed the registry; see `system-knowhow/intent-registry.md`. There is no top-level `data/intents/` source. |
| Scripts | `data/scripts/`, `data/apps/<id>/scripts/`, `data/triggers/<slug>/scripts/`, `data/knowhow/<id>/scripts/` |
| Authored plugin trees | any `manifest.toml` under `data/` that is a plugin root (has `id`, `version`, `name`, `description`) |
| Installed plugins | the `PluginInstalled` / `PluginUninstalled` event projection, not a `data/` path |
| Artifacts (structural only) | `data/artifacts/` |

Trigger intent text lives in the `TriggerCreated` event payload (`run.intent`), not on disk. Pull it with `lucidos events query --type TriggerCreated`.

## The mechanical scan: run this first, verbatim

A grep decides every pattern below, not your judgment. Run them as ONE call,
before reading any file, so coverage is a command that ran or did not. From
memory, a check gets skipped, and a skipped check reads like a clean one.

`run_bash` starts in the workspace root, so this needs no absolute path.

Each section prints its hit count, and the tail repeats them as one table. That
table is the **receipt**: proof the section ran, and the only honest basis for
calling a category clean.

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
| `theme-rename` | names from before *look* became *theme* | 2 |
| `hand-rolled-ui` | a control the app draws itself that the SDK provides | 2 |
| `attr-escape` | a text-only escaper writing into an attribute value | 2 |
| `ready-signal` | the manifest's `reveal`, and calls to `lucidos.ui.ready()` | 2 |
| `app-icon` | the manifest's `icon` | 2 |
| `tap-strings` | the retired `tap` string forms | 1 |
| `removed-flags` | CLI flags and tool args that were removed | 8 |
| `removed-fields` | thread-summary fields the engine no longer sends | 8 |
| `cred-env` | a credential read from the environment | 8 |
| `auth-header` | an auth header built in app UI code | 8 |
| `machine-path` | a hardcoded home or machine path | 5 |
| `plugin-manifests` | authored plugin-root `manifest.toml` files and whether each declares `engine` | 6 |

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

scan theme-rename "data-theme([^-]|\$)|data-look-|look-effects|lucidos-look|/looks?[/?'\"\`]|preferences\.(set|get)\(['\"](look|theme)['\"]" $inc --include=*.css apps

scan hand-rolled-ui "(^|[^.[:alnum:]_])(alert|confirm|prompt)\(|window\.(alert|confirm|prompt)\(|function[[:space:]]+(toast|showToast|snackbar)[[:space:]]*\(|class=[\"'][^\"']*(toast|snackbar)|^[[:space:]]*\.(toast|snackbar)[[:space:]{.,:]|<select[[:space:]>]|[[:space:]]title=[\"']|role=[\"']switch|(toggle|switch)[[:alnum:]_-]*(thumb|knob)|^[[:space:]]*\.[[:alnum:]_-]*(spinner|loader|badge|chip|pill|tabs?)[[:space:]{.,:]|role=[\"']tab[\"']|^[[:space:]]*(input|textarea)[[:space:]{.,:[]" $inc --include=*.css --exclude-dir=tests apps

scan attr-escape "=[[:space:]]*[\"'](\\\$\{|[\"'][[:space:]]*\+)[^}]*escapeHtml\(" $inc apps

scan ready-signal "\"reveal\"[[:space:]]*:|lucidos\.ui\.ready\(" --include=manifest.json $inc apps

scan app-icon "\"icon\"[[:space:]]*:" --include=manifest.json apps

scan tap-strings "\"tap\"[[:space:]]*:[[:space:]]*\"(modal|none|open_app|open_thread)\"|tap:[[:space:]]*'(modal|none|open_app|open_thread)'|kind:[[:space:]]*'none'" $all

scan removed-flags "spawn-thread[^|]*--(parent|cc-model)|run_coding_agent\([^)]*repo[[:space:]]*=|threads (list|count)[^|]*--has-diff" $all

scan removed-fields "coding_agent_(applying|has_diff|proposed|incomplete|requires_restart)" $all

scan cred-env "CRED_[A-Z0-9_]+" --exclude-dir=auth $all

scan auth-header "Authorization|X-API-Key" $inc apps

scan machine-path "/(Users|home)/[^\"'[:space:]]+" $all

plugin_manifests=""
while IFS= read -r -d '' m; do
  has_id=$(grep -qE '^id[[:space:]]*=' "$m" && echo y)
  has_version=$(grep -qE '^version[[:space:]]*=' "$m" && echo y)
  has_name=$(grep -qE '^name[[:space:]]*=' "$m" && echo y)
  has_desc=$(grep -qE '^description[[:space:]]*=' "$m" && echo y)
  if [ "$has_id" = y ] && [ "$has_version" = y ] && [ "$has_name" = y ] && [ "$has_desc" = y ]; then
    if grep -qE '^engine[[:space:]]*=' "$m"; then
      tag="engine declared"
    else
      tag="no engine"
    fi
    plugin_manifests="$plugin_manifests$m ($tag)
"
  fi
done < <(find . -name manifest.toml -not -path '*/node_modules/*' -print0 2>/dev/null)
n=$(printf '%s' "$plugin_manifests" | grep -c . || true)
printf '\n=== plugin-manifests (%s hits) ===\n%s\n' "$n" "${plugin_manifests:-none}"
receipts="$receipts| plugin-manifests | $n |\n"

printf '\n=== RECEIPTS: copy this table into the report ===\n'
printf '| section | hits |\n|---|---|\n'
printf '%b' "$receipts"
```

**A hit is evidence, not a finding.** The owning check says what a hit means,
how bad it is, and what to recommend. Some hits are fine in context: `apiUrl`
building a `src` is correct, and a `try` around a storage call changes the
severity without clearing it. Report the hits that stand.

**The receipts table goes in the report, verbatim.** A `0` is a result: the
category is clean. A section MISSING from the table is a category nobody looked
at. Say so in the report rather than dropping the row, because a reader counts an
absent row as clean.

## What to check

For each finding, capture: **location**, **what's wrong**, **which reference owns the rule** (link, don't quote), **suggested fix**.

### 1. Triggers: intent vs knowhow split

**Start from the live projection (`list_triggers` / the trigger registry the scheduler uses), not from `TriggerCreated` events.** Anything off the live list was deleted (`TriggerDeleted`) or no longer fires, so walking events alone invents phantom "broken" triggers. Use `TriggerCreated`/`TriggerUpdated` only to rebuild `run` fields the projection hides (intent text, stale `run.knowhow:[...]`, slug) for live triggers.

Per the engine prompt's taxonomy section and the worked example in `docs/taxonomy.md` (mirrored in best-practices). Trigger threads discover knowhow at fire time via `load_knowhow`, as chat does, with no per-trigger allow-list. For each *live* trigger, reduce its `TriggerCreated` and later `TriggerUpdated` events to the *latest* `run` (most recent payload by sequence), then apply these checks:

- **Imperative verbs about *how* in `run.intent`** (hit, parse, scan, fall back, retry, GET, POST, scrape) → procedure leaked into intent.

- **Stale `run.knowhow: [...]` field.** Per `system-knowhow/triggers.md` § "Setup checklist" item 4: the deserializer silently drops legacy `run.knowhow:[...]`. The trigger keeps firing with no knowhow pre-loaded, so behavior depends on discovery finding the same files. Surface each trigger's id and the `knowhow` ids it requested, and recommend the rewrite the source file specifies. Severity: **stale** (silently broken).

- **Subscription on a retired event name.** For each live trigger's `on` list, classify every `event_type` with the `events` tool's `event_types` action. One call returns all three buckets.

  **A rename does not carry subscriptions with it.** The matcher compares the event type as an exact string, so a trigger on a retired name silently stops firing. Only an audit finds these.

  | Bucket the name is in | Verdict |
  |---|---|
  | `engine` | fine, it is live |
  | `retired` | **broken**, it silently stopped firing. Re-point it, see below |
  | `workspace` | fine, it is a domain event this workspace emits itself |
  | none of the three | **smell**, a subscription on a name nothing produces |

  Domain-event names are arbitrary by design, so read the `workspace` bucket first, and never call one a misspelling.

  **Ask the tool. Never assemble the retired set by hand.** `retired` comes from `ThreadEvent::LEGACY_TYPE_NAME_ALIASES`, which a test holds to the names serde still accepts. A list built by eye from `thread-events.md` misses about half, and adds frontend-only names that were never event renames.

  **`retired` says a name is dead, not what replaced it.** For the successor, search `thread-events.md` for the old name. It sits on its successor's row, as a `Legacy alias:` note or in the row's prose. The whole `ClaudeCode*` family became `CodingAgent*`, as `system-knowhow/coding-agent-events.md` states. If neither resolves it, report the finding without a replacement. A guessed successor arms a second subscription that never fires.

  The engine now refuses a new subscription on a dead name, so this check finds rows armed before that.

- **Retired event names in workspace code.** Same list, different surface, because a stale recipe keeps minting dead subscriptions and the finding above returns after the fix. Grep for retired names in `await_event` calls, `on_event` payloads, and `lucidos triggers` invocations. Search `data/knowhow/**/*.md` (fenced `python` / `bash` / `js` / `ts` blocks), `data/scripts/**`, `data/apps/**`, and `data/triggers/**/scripts/**`. Surface path, line, the retired name and its current name, at severity **stale**.

- **Missing explicit `slug` field.** The tools, CLI and HTTP API store the slug on `TriggerCreated` (`system-knowhow/triggers.md` § "Setup checklist" item 4). An older event without one re-derives it from the *create-time* name on every read, with no collision check. Recommend an explicit `slug` when the trigger has, or will have, per-trigger knowhow files. Set it with `lucidos triggers update --slug` or the HTTP API, since the LLM tools take none. Severity: **nit** (preventive).

- **Per-trigger knowhow dir orphaned from any live trigger**: for each directory under `data/triggers/<slug>/knowhow/`, confirm `<slug>` matches an active (non-deleted) trigger's slug. The system prompt scopes by exact slug match, so knowhow under an unreferenced slug is invisible. Recommend renaming the directory to a live slug or deleting it. If the trigger runs a **script**, moving its folder also needs `update_trigger(run.path=…)`, since the registered path does not follow the folder. Deleting the old folder before that event lands breaks the next fire (see `triggers.md` § "Renamed trigger → stale `run.path`"). Reference: `system-knowhow/triggers.md`.

- **Notification routing: `tap` opt-ins for CTA-shaped triggers.** Take each trigger whose `run.intent` mentions `send_notification`, plus each `NotificationCreated` event traceable to a trigger. Look the body's shape up in the table under `system-knowhow/triggers.md` § "Notification routing". Report each trigger whose `app_id`, `tap` or `event_id` disagrees with its row, quoting the row as the fix. Severity: **drift**, since the default works and the opt-in only tightens UX. Skip a trigger that already sets `tap` to a non-default value.

- **Old-form `tap` strings.** The field was a four-string union and is now a discriminated union object. The engine rejects the strings with `400 Bad Request` at write time. Grep every code-bearing surface in § "What to walk": trigger scripts, app code (`ui/`, `*.html`, inline `<script>`), shared scripts, and fenced `python` / `bash` / `js` / `ts` blocks in knowhow. Skip `data/artifacts/` and `data/postgres/`, which hold no code that calls the API.

  Match each form in both quote styles: the key-quoted `"tap":` spelling (Python, shell, JSON bodies) and the bare `tap:` spelling (JS, TS):

  - `tap: 'modal'`
  - `tap: 'none'`
  - `tap: 'open_app'`
  - `tap: 'open_thread'`
  - `{ kind: 'none' }`, the retired object form

  Surface path, line and the matched form. Severity: **broken**, because the next fire 400s. The one exception is the retired `{ kind: 'none' }` object. The engine coerces it to `{ kind: 'modal' }`, so it is **stale**: it runs, but still spreads by copy. Canonical `Tap` type: `system-knowhow/js-sdk.md` § `lucidos.notifications`.

  URL-encoded and hash-form taps are **out of scope**. The engine and the service worker own that channel, not any workspace file.

  § Remediation carries the old-to-new mapping a fix thread needs.

### 2. Apps: SDK boilerplate and structure

Per `system-knowhow/js-sdk.md`:

- `index.html` matches the current boilerplate (script order, which pieces are required vs optional).
- Every `lucidos.*` call used in app code appears in the SDK reference. Calls not listed are either deprecated or invented.
- **External-API calls from the iframe: USE `lucidos.proxy(name).fetch(path, init)`.** The engine forwards the request server-side, strips this side's headers, and injects the configured auth header from the credential store. The credential never reaches the iframe. Configure the backend once in `data/config/apis.json`. Reference: `system-knowhow/js-sdk.md` § `lucidos.proxy`, which owns the strip list.

  **DO NOT USE** either of the following. Flag each occurrence and recommend the SDK helper:

  - `fetch('http://...')` or `fetch('https://<external-host>/...')` from inside an iframe. Mixed-content / CORS blocks it; if it works, the credential is sitting in the iframe. Suggest a `data/config/apis.json` entry and `lucidos.proxy(name).fetch(...)`.
  - `fetch('/api/v1/proxy/<name>/...')`: the SDK helper's wire format, bypassing the helper. The proxy name becomes a typo-prone magic string, and future SDK-side handling (timeouts, retries, response parsing, error shape) won't apply. Suggest `lucidos.proxy('<name>').fetch(path, init)`.

  A credential header written into app code belongs to check 8.

- **The app calling the engine with its own `fetch`.** Walk `data/apps/**/*.{js,ts,html}` for any engine call JavaScript makes itself: `fetch('/api/v1/events/query')`, `new URL('api/v1/events/query', document.baseURI)`, `new EventSource('/api/v1/events')`, `fetch(lucidos.apiUrl('/<suffix>'))`, and any `location.pathname`-splicing that rebuilds the workspace address. In the host shell none reaches the engine: the frame's origin is opaque and CORS refuses it. Severity **broken**. WebKit reports `Load failed` and Chromium a `TypeError`. The failure usually lands in a falling-back `catch`, so the symptom is stale data, not an error.

  **`lucidos.apiUrl` is in that list deliberately.** Where an SDK method covers the endpoint, name that method. Otherwise the remedy is `lucidos.request('/<suffix>', init)`, which travels the bridge (ADR 0231). Never recommend `apiUrl` for a call: that moves a working app onto a pattern that cannot run. Reference: `system-knowhow/js-sdk.md` § `lucidos.request`.

  **A route an app may not reach has no remedy, and saying so is the finding.** The engine classifies every route, and `lucidos.request` refuses the rest with a 403 in both realms. Credentials, thread contents, consent routes and platform control are denied on purpose. Report the call and what it reaches for, and invent no way around. Reference: `system-knowhow/js-sdk.md` § "Not every endpoint is reachable".

  An `/api/v1/` path in a markup `src` / `href` attribute is correct and must NOT be flagged, nor is `apiUrl` building one. Five are exempt from the gateway's device gate: `sdk.js`, `sdk-prefs.js`, `sdk-iframe.css`, `sdk-iframe-audio.js`, and anything under `fonts/`. Behind a gateway, any OTHER `/api/v1/` path in a tag is refused: the frame's pass to its own files reaches no engine route. Severity: **broken**, and the remedy is the `lucidos.*` method that covers it.

- **What an isolated app frame can no longer do.** In the host shell an app runs at an opaque origin, in its own renderer process, so it cannot freeze or read the shell. The price: the frame's own `fetch`, `EventSource` and browser storage all fail, and the SDK carries those three over a bridge. Reference: `system-knowhow/js-sdk.md` § Setup, and *app frame* / *app bridge* in `docs/glossary.md`. Walk `data/apps/**/*.{js,ts,html}` for code that goes around it, skipping any vendored `node_modules/` tree:

  - **Browser storage touched directly**: `localStorage`, `sessionStorage`, `document.cookie`, `indexedDB`. Each throws a `SecurityError` in the frame. The remedy is in § Rewriting an app for an isolated frame. Report the two shapes apart, because they fail differently:
    - **broken** where the call sits outside a `try`. The throw stops the script, so the app renders nothing.
    - **stale** where a `try` / `catch` wraps it, the common shape written for Safari private mode. The app runs and silently stops remembering anything.
  - **A read of the host realm**: `window.parent`, `parent.document`, `window.top`, or the device id lifted out of the shell's storage. All blocked. Severity: **broken**.
  - **`<a href="<the app's own file>" download>`**, in an app whose `index.html` loads no `/api/v1/sdk.js`. A browser ignores `download` on a cross-origin link, so the click navigates the frame to the file. Severity: **broken**.
  - **The camera or the microphone.** `navigator.mediaDevices.getUserMedia`. Both browsers refuse media capture to an opaque origin, whatever the frame is granted, so no in-frame remedy exists. Severity: **broken**. Say the app has to run in its own tab. Reference: `system-knowhow/js-sdk.md` § Setup, which lists what the frame is granted.
  - **The OS share sheet.** `navigator.share` called directly. The frame is not granted `web-share`, and iOS refuses the delegation anyway. Severity: **broken**. The remedy is `lucidos.ui.openExternal(url)`, which opens the link through the host. A hit inside a vendored `sdk.js` is the SDK's own fallback, so read the call site first.
  - **Session-history URL writes.** `history.replaceState`, `history.pushState`, or assignment to `location.href` / `window.location` / `location.assign()` / `location.replace()`. The frame is sandboxed without `allow-same-origin`. So the browser refuses any session-history URL write whose path or fragment differs from the frame's real URL. The error reads "Paths and fragments must match for a sandboxed document". Split the severity the way the failure does:
    - **broken** where the call sits outside a `try`. It throws, and anywhere in the render or boot path the app renders nothing.
    - **stale** where a `try` / `catch` wraps it. The URL stops reflecting state, so deep links out of the app break while nothing looks wrong.

    The remedy: an app must not write its own URL, and has no way to. Reading a fragment still works: apply `location.hash` at boot and subscribe to `hashchange`, which is how inbound deep links arrive. To share its state, the app builds the link string and copies or shows it. Reference: `system-knowhow/js-sdk.md` § Setup (what the frame is granted) and § "fragment: opening at a place inside the app".
  - **A write to `/env-vars`**, through `lucidos.request` or the app's own `fetch`. The route opens `GET` only. A user env var reaches every command the agent runs, so a name the interpreter loads from would be host code execution. Severity: **broken**, since the call answers 403 and the app's settings never persist. § Remediation carries the replacement.
  - **A write of a human-only preference**, through `lucidos.preferences.set` or a `PUT /preferences`. The route stays open, but two key classes are refused to an app. One the Lucidos Agent may not write either: `command_guard`, `max_tool_calls`, `network_bind`, `local_base_url`, the judge settings and the `provider_enabled_*` switches. One the agent may write but an app may not: the coding-agent paths and permission mode. These would let an app run its own script as the user, or read local-model chat. Severity: **broken** (403); § Remediation carries the answer.
  - **A read or write of engine bookkeeping**, such as the Web Push keypair in `vapid_keys`. A preference read leaves it out, and an app's `PUT /preferences` naming it is refused. Severity: **broken** (403 on write, a missing key on read); § Remediation carries the answer.
  - **The app's own bundled file, fetched by a relative path.** `fetch('data/song.json')`, ``fetch(`audio/clips/${name}.json`)``, any `fetch` whose first argument is a relative path. The frame's origin is opaque, so the browser refuses it like an engine call: `Load failed` in WebKit, a `TypeError` in Chromium. It reads as safe, and the engine-fetch pattern misses it, since nothing in the string says `/api/v1`. Split the severity the way the failure does:
    - **broken** where the throw escapes setup, or the fetch is the app's only data path. The app renders an error banner, or nothing.
    - **stale** where a `catch` falls back to `lucidos.data` and the app keeps running, minus whatever the bundled file carried.

    The remedy is `lucidos.data.read('apps/<app-id>/<path>')`, which travels the bridge. **`lucidos.data.url()` is not a remedy.** It builds a URL for a `src` or an `href`, and fetching one is refused the same way.
  - **A `<base href>` the app declares itself.** The first base in a document wins, so it replaces the pass the engine stamps for the app's own files. Behind a gateway every relative `src` / `href` then answers **401**. Severity: **broken**. Delete it: relative refs already resolve against the app's own directory. Reference: `system-knowhow/js-sdk.md` § Setup, and [ADR 0238](https://github.com/lucidos-dev/lucidos/blob/main/docs/adr/0238-app-frame-carries-a-capability-to-its-own-files.md).

  **A separate `app.js`, `style.css` or image is NOT a finding.** The engine gives each framed document a short-lived pass to its own files. Do not flag one, and do not recommend inlining. The same holds for the app's own `@font-face` and `<script type="module">`. The engine grants both across the opaque origin, the module behind the gateway every install runs ([ADR 0289](https://github.com/lucidos-dev/lucidos/blob/main/docs/adr/0289-app-frames-load-fonts-and-modules-across-origins.md)).

  An app opened in its own tab keeps all of this, as a top-level document. Never report it unaffected on that basis: the same app is reachable both ways, and the frame is the usual one.

- **A text-only escaper writing into an attribute value.** The `attr-escape`
  scan finds `escapeHtml(...)` spliced into a quoted attribute, as in
  `` `<a title="${lucidos.utils.escapeHtml(x)}">` ``. That helper leaves `"`
  and `'` raw, so a quote in the value breaks out and runs script with the
  app's full authority. Severity: **broken**. The remedy is
  `lucidos.utils.escapeHtmlAttr`. Reference: `system-knowhow/js-sdk.md`
  § "Escaping: which helper goes where".

  Read the call site first. An app may define its own `escapeHtml` that also
  escapes both quotes, and that one is correct.

- **Names from before the theme rename.** *Look* became *theme*, and the
  light/dark `theme` preference became `theme-mode` (ADR 0316). The `theme-rename`
  scan finds the old names. Judge each hit:
  - **broken**: `data-theme` in a selector or an attribute read. An app frame
    no longer carries it, so those styles never match. Recommend
    `data-theme-mode`.
  - **broken**: `data-look-effects`, `data-look-parts`, the `look` or
    `look-effects` preference, or a `/looks` or `/look?id=` route. None of
    them exists any more. Recommend `data-theme-effects`, `data-theme-parts`,
    the `theme` and `theme-effects` preferences, and `/themes` or `/theme?id=`.
  - **broken**: `theme` set to `light`, `dark` or `system`. The engine refuses
    it. Recommend `theme-mode`. A `theme` set to a theme id is correct.

- **A control the app draws itself that the SDK provides.** The host draws the
  SDK's helpers, so they pick up every later fix and theme change. An app's own
  copy stays as it was the day someone wrote it. The `hand-rolled-ui` scan finds
  the copies. Judge each hit:
  - **drift**: the browser's own `alert()`, `confirm()` or `prompt()`. They
    still work in the frame, unthemed.
  - **drift**: the app's own toast or snackbar, meaning a `toast()` function or
    a `.toast` element with its own CSS.
  - **drift**: a `<select>` in an app that never calls `lucidos.ui.Select` or
    `lucidos.ui.enhanceSelects`.
  - **drift**: a `title="…"` attribute used as a tooltip. It never shows on a
    touch device.
  - **drift**: an on/off switch the app draws itself, such as a `role="switch"`
    button with its own track and thumb. A `role="switch"` checkbox inside a
    `.toggle-switch` label is the shared one and is correct.
  - **drift**: a spinner the app draws itself, meaning a `.spinner` or
    `.loader` rule of its own. Where it stands in for loaded data, say so: data
    loading draws a skeleton, and only a working state takes the ring.
  - **drift**: a badge, chip or pill the app draws itself to show a status or
    a category. A pill the user taps to filter or switch views is the next case.
  - **drift**: tabs or filter pills the app draws itself. A `role="tab"` button
    with the `pill-bar-btn` class is the shared one and is correct.
  - **drift**: a bare `input`/`textarea` CSS rule giving it its own border or
    background. The `.text-input` class already is correct and is not a hit.

  Read the call site before you report a hit. An app can define its own
  `confirm()` method, and a `title` on an `<iframe>` or `<abbr>` is an
  accessible name, not a tooltip. Leave those.

  **A pattern with no SDK counterpart is not a finding.** Cards and custom
  modals have none yet, so an app that draws its own is correct. Owns the rule:
  `system-knowhow/js-sdk.md` § Toasts, § Confirmation dialogs, § Prompts,
  § Tooltips, § lucidos.ui.Select and § Component classes. § Remediation carries
  the replacements.

- **The ready signal.** An app can hold the host's loading cover until its
  content is drawn: `"reveal": "on-ready"` in `manifest.json`, then a
  `lucidos.ui.ready()` call. The `ready-signal` scan lists both halves per
  app. Read them together, plus the app's startup code. Owns the rule:
  `system-knowhow/js-sdk.md` § Showing the app once its content is ready.
  - **broken**: the manifest says `on-ready` and no app code calls
    `lucidos.ui.ready()`. Every open sits behind the cover for the full
    15 s fuse.
  - **broken**: `reveal` holds any value other than `on-load` or `on-ready`.
    The app opens as `on-load`, so the opt-in the author meant is ignored.
  - **stale**: the app calls `lucidos.ui.ready()` but its manifest does not
    say `on-ready`. The call does nothing.
  - **stale**: the call exists, but a startup path that finishes the first
    render skips it, typically the `catch` that shows an error. That path
    waits for the fuse.
  - **nit**: no opt-in, and the startup code awaits `lucidos.data`,
    `lucidos.events`, `lucidos.proxy` or `lucidos.request` before its first
    render. The user sees an empty app until the data lands. Recommend the
    opt-in. Skip an app that draws a skeleton or its own placeholder first.

Per `system-knowhow/building-an-app.md` § App icon:

- **The app icon.** The `app-icon` scan lists each manifest's `icon`. Check
  each value against the rule that section states.
  - **stale**: `icon` breaks that rule. The app shows its monogram tile, so
    nothing breaks, but the author meant an icon.
  - **nit**: an app or widget with no `icon`. Recommend drawing one.

Per `system-knowhow/building-an-app.md` § Widgets:

- **A widget's origin thread.** A manifest with `"kind": "widget"` must carry an `origin_thread_id` that names a thread in this workspace (`lucidos threads list`, or `thread_summaries`). A `reusable` widget is exempt from the thread check, since no thread owns it, but still needs the field.
  - **broken**: `origin_thread_id` is missing or not a uuid. The widget shows on no shelf it can find its way back to, and no Delete (a thread) will ever remove it.
  - **stale**: it names a thread that no longer exists, and the widget is not `reusable`. Nothing shows it or removes it. Suggest making it reusable or removing the folder.
  - **broken**: `kind` holds a value other than `app` or `widget`. The folder lists as an app, which is not what the author meant.

Per `system-knowhow/best-practices.md`:

- `manifest.json` carries only user-facing metadata; no operational knowledge has leaked in.
- Single-app docs live under `apps/<id>/knowhow/`; multi-consumer docs live under shared `data/knowhow/`.
- App data persists under `data/artifacts/<app-id>/`.

### 3. Knowhow: naming, frontmatter, and content

Per `docs/taxonomy.md` (frontmatter shape) and `system-knowhow/best-practices.md` (file placement):

- Frontmatter has `name` (required) and `description` (recommended, since semantic discovery uses it).
- Filename is descriptive, not generic.
- App-scoped knowhow doesn't reference things outside its app; shared knowhow doesn't name specific apps.
- **A file below the listing depth that no doc names.** Per `system-knowhow/building-knowhow.md` § "Where the file goes" (mirrored in `docs/taxonomy.md` § "Knowhow: Docs and References"), a root lists `data/knowhow/<name>.md` and `data/knowhow/<group>/<name>.md`. Under `data/apps/<id>/knowhow/` and `data/triggers/<slug>/knowhow/` it lists one level only. A deeper file is a *reference* belonging to the doc above it.

  That shape is legitimate, so never flag depth alone. Flag only a reference **no doc names**: grep the sibling docs for its full id (the path under the root without `.md`). A named one is correct.

  An unnamed one is unreachable: it sits in no routing list, and nothing tells the LLM the id exists. Recommend naming it from the doc that should own it, or moving it up to the listed depth. Severity: **stale** (silently invisible). Nothing fails at runtime, so only this check reveals it.
- **Orphaned files under `data/knowhow/`**: a file no consumer names is potentially dead. Its id appears in NO trigger's stale `run.knowhow` (see § 1), NO intent's `knowhow:` frontmatter (see § 4), and NO app `manifest.json`/`config/*.json`. The usual cause is a trigger that lost `run.knowhow` when the preload was retired, with the content never moved into its intent. Recommend one fix: (a) inline the procedure into a trigger's intent, (b) move the file into `data/triggers/<slug>/knowhow/` if trigger-specific, or (c) delete it. Severity: **stale** (review). Reference: `system-knowhow/triggers.md`.

Knowhow bodies also carry pre-proxy API patterns. Run check 8 over the fenced code blocks here too. Knowhow is where a leak *spreads*: it tells the next session how to call the API, so the finding returns after the code is clean.

### 4. Intents: frontmatter and tone

Scope is **every `.md` file the registry reads** (per `system-knowhow/intent-registry.md`): `apps/<id>/intents/`, `apps/<id>/triggers/`, and `triggers/<slug>/`. Trigger `.md` files are intents too. If an ID in the engine's "Available Intents" list has no file under `intents/`, look in the sibling `triggers/` directory before calling it a phantom.

- `name` present.
- `knowhow:` IDs in the frontmatter (if any) resolve to existing files. Severity **broken**. An ID is the path under `data/knowhow/` (or `system-knowhow/`) without `.md`, INCLUDING any subdirectory. The usual drift is a bare basename for a file in a subdirectory: `'nightly-pipeline-trigger'` for `data/knowhow/lucidos-ops/nightly-pipeline-trigger.md` (correct id: `lucidos-ops/nightly-pipeline-trigger`). Resolve each id against the full relative path under `data/knowhow/` (and `system-knowhow/` for prefixed ids).
- Reads in user terms, not engineer terms (same test as triggers).
- **Do not flag** a missing `data/triggers/<slug>/<slug>.md` for a *standalone scheduled trigger*. Its `run.intent` (in the `TriggerCreated` payload) is enough for scheduled firing. An on-disk procedure file is warranted only for dual use: scheduled firing **and** on-demand `execute_intent`. A pure scheduled orchestrator nothing calls manually is correct as-is.

### 5. Scripts: CLI usage and isolation

Per `system-knowhow/lucidos-cli.md`:

- Writes to `data/` go through `lucidos data write`, not raw HTTP and not open-coded paths under `$LUCIDOS_WORKSPACE/data/`. Do not flag a script's own runtime state (a cursor, a last-seen id) written directly under `data/artifacts/<plugin-id>/` or `data/triggers/<slug>/state/`. That is the intended pattern, per `system-knowhow/plugins.md` § "Where a plugin keeps its runtime state".
- Domain events go through `lucidos events emit` / `lucidos events query`.
- External API calls go through `lucidos proxy <name>` when the workspace owns a credential for the service. The patterns are check 8's. A script adds one consequence: a credential in argv also lands in shell history.
- No hardcoded absolute paths to a specific workspace.

Per `system-knowhow/best-practices.md`:

- Script lives with its sole consumer (single-app script in `apps/<id>/scripts/`, not shared `data/scripts/`).

### 6. Plugins: the `engine` requirement

Two surfaces, read differently. Per `system-knowhow/plugins.md` § "The `engine` requirement".

**a. Installed plugins: not a grep, a tool call.** Call `plugins(action="check_updates")` (no `id`) once, every pass, and add a `plugins-engine` row with the count checked to the receipts. Per installed plugin, it reports `installed_version`, `latest_version`, `changed`, `source`, and the **latest** version's `engine_requirement` / `engine_compatible` / `engine_incompatible_reason`.

That `engine_requirement` is the remote manifest's, not the installed one's (`crates/lucidos-engine/src/engine/tools/plugins/registry.rs::execute_check_plugin_updates` reads the freshly fetched manifest). The installed declaration lives on the `PluginInstalled` event. Query the `events` tool for `event_type="PluginInstalled"`. Keep the latest per plugin id with no later `PluginUninstalled`, as check 1 reduces triggers. Read `payload.data.manifest.manifest.engine`, the nested path `system-knowhow/plugins.md` § `PluginInstalled` documents. Flag its absence, never the latest's.

For each installed plugin whose own `engine` is absent:

- A newer version exists, declares `engine`, and `engine_compatible` is true: finding. Fix: update the plugin (`plugins(action="update", id=...)`), which stages the confirm panel for the user to accept. Severity: **nit** (nothing is broken yet; the common case once an author adds a floor).
- A newer version exists but `engine_compatible` is false: finding, quoting `engine_incompatible_reason`. Fix: update Lucidos, then retry the plugin update. Severity: **nit**.
- The same `PluginInstalled` event has no `source` either (an archive install): finding. Fix: ask whoever shared the plugin for an updated archive that declares `engine`. Severity: **nit**. Check this before the `error` row below, where a sourceless plugin also shows up, less specifically.
- No newer version declares one (no update exists, or the latest manifest also lacks `engine`): finding. Fix: set `engine` to the first Lucidos release shipping every platform feature the plugin uses. `system-knowhow/plugins.md` § "The `engine` requirement" says how to find it. If `source` is a repo the user can push to, add the floor there; otherwise use "Proposing your patch upstream" in `system-knowhow/plugins.md`. Severity: **nit**.
- A `check_updates` entry carries any other `error` (a fetch failure, not the sourceless case): report "could not check `<id>`'s update, `<error text>`", never skip it. Severity: **smell** (the audit's coverage of this plugin is incomplete, not necessarily the plugin).

**Never edit an installed plugin's files under `data/` to add `engine`.** `manifest.toml` never lands under `data/`, and the floor is read from the fetched source. See `system-knowhow/plugins.md` § "The `engine` requirement".

**b. Plugin trees the workspace authors.** The `plugin-manifests` scan lists every `manifest.toml` under `data/` with `id`, `version`, `name` and `description` (a plugin root), and whether it declares `engine`:

- No `engine` key: finding. Fix: set `engine` to the first Lucidos release shipping every platform feature the plugin uses (`system-knowhow/plugins.md` § "The `engine` requirement" says how to find it). Bump `version` too, so existing installs receive the change. Severity: **nit**.
- An `engine` value that is not a valid semver requirement (not a string, empty, or a typo such as `"latest"`): read the file to confirm. Severity: **broken** (install refuses the plugin, so it cannot ship until fixed). Fix: same as above.

### 7. Artifacts: structural only

Don't enumerate user content. Per `system-knowhow/best-practices.md`:

- No `data/artifacts/artifacts/`.
- No `data/artifacts/themes/` (or `data/artifacts/looks/`, its name before the rename), `data/artifacts/config/`, `data/artifacts/auth-modules/` or `data/artifacts/scripts/`. `lucidos data write` once filed those trees under `artifacts/`, as the agent's file tools did for themes. Nothing reads them there: a theme never shows, and an `apis.json` never loads. Severity: **broken**. Owns the rule: `system-knowhow/lucidos-cli.md` § `lucidos data path`.
  - Flag a file only when its content fits the tree: a theme JSON, `apis.json`, a signer `.wasm`, or a handshake script. An artifact project that shares the name is not a finding.
  - Recommend writing each file again at its real path with `lucidos data write`, without the `artifacts/` segment, which also runs the engine's checks. Then delete the copy under `artifacts/`.
- No HTML artifact that expects the shell's authority. A previewed or served HTML file runs sandboxed at an opaque origin. Its engine calls, its `fetch()` of sibling files and its browser storage all fail. Grep the artifact's own inline `<script>` blocks in `data/artifacts/**/*.html` for `/api/v1`, `new EventSource(`, `fetch(`, `localStorage`, `sessionStorage` and `parent.document`. Skip a vendored library file.
  - An unguarded storage access throws and stops the script. Severity: **broken**. A `fetch` inside a `catch` that falls back leaves an empty or stale report. Severity: **stale**.
  - Recommend writing the data into the file when it is written, or making it an app.
  - Owns the rule: `system-knowhow/best-practices.md` § What a standalone HTML document can do.
- No bulk imports under `data/artifacts/imported/<service>/` that match the "dumped repo / archive" anti-pattern (file count + size are the tell). Suggest moving bulk to `.lucidos/tmp/` or `~/.lucidos/data/`.
- No orphaned `imported/<service>/` directories. Flag for review (don't auto-delete).
- App data sits under `data/artifacts/<app-id>/`, not at the artifacts root.

### 8. Cross-cutting

- **A credential written into workspace code.** One rule over four surfaces, so it lives here, not in checks 2, 3 and 5. Where the workspace calls an external API the engine holds a credential for, the credential belongs in the credential store and the backend in `data/config/apis.json`. Flag:

  - An inline auth header. `curl -H "Authorization: Bearer $CRED_<NAME>"`, a pasted literal token, an `Authorization` / `X-API-Key` / `Bearer` header built in JS, or `requests.get(url, headers={"Authorization": ...})` and its equivalents.
  - Prose telling a future session to "set `$CRED_X`", or to read the credential out of the environment.

  Walk `data/apps/**`, `data/scripts/**`, every `scripts/` under a trigger, and the fenced `bash` / `sh` / `python` / `js` / `ts` blocks in `data/knowhow/**/*.md`.

  The fix differs only by caller: `lucidos.proxy(name).fetch(...)` in an app, `lucidos proxy <name>` in a script, `proxy_request` for the LLM. Severity: **drift**, since the call still works. Give a pasted literal token its own line in the report: the file is git-tracked, so the credential needs rotating, not just rerouting. Owns the rule: `system-knowhow/js-sdk.md` § `lucidos.proxy` and `system-knowhow/lucidos-cli.md` § `lucidos proxy`.

- Broken references: missing `knowhow:` ID, missing script path, manifest pointing at a deleted asset.
- Duplicated content: same knowhow text in two files, same script copied between apps.
- Patterns the source-of-truth files explicitly mark deprecated. Grep for the old form and point at the doc that flags it.
- **Removed CLI flags and tool args still passed by workspace code.** These were
  removed after a deprecation window, and a recipe passing one now fails with a
  rename error. Grep each form across `data/knowhow/**/*.md` fenced code blocks,
  `data/scripts/**`, each trigger's `scripts/`, and `data/apps/**`, and recommend
  the replacement. Severity: **broken** (the call errors on next use). Currently
  removed:
  - `lucidos spawn-thread --parent` → `--relation child` (a same-workspace
    parent-with-callback spawn). Do NOT flag `threads list --parent <uuid>` or
    `threads count --parent <uuid>`, which are current, unrelated filters.
  - `lucidos spawn-thread --cc-model` → `--coding-agent-model` (same value).
  - `repo` passed to the `run_coding_agent` tool → `folder` (which also accepts a
    registered repo name). Do NOT flag the current `lucidos spawn-thread --repo`
    flag, which is a different, live argument.
  - `lucidos threads list --has-diff` and `threads count --has-diff`, and the
    `threads` tool's `has_diff` arg → `--change-state` / `change_state`. The
    tool and the HTTP route refuse a stale `has_diff` by name.
    `--has-diff false` becomes `--change-state none`. `--has-diff` covered both
    `unproposed` and `proposed`, so ask which one the code means. The scan
    finds only the CLI form: read knowhow `threads` calls for `has_diff` by
    hand. Do NOT flag `has_diff` on `CodingAgentDiffChanged`, which is current.
- **Removed thread-summary fields still read by workspace code.** A thread
  summary comes from `lucidos.threads.list`, `lucidos threads list` or the
  `list_threads` tool. A removed field reads as `undefined` in JS, so the code
  quietly takes its falsy branch. Grep the same four surfaces as the flags above. Severity: **stale**, or **broken** for a Python `row["field"]`,
  which raises on the missing key. Currently removed:
  - `coding_agent_applying`. There is no replacement: it never tracked a live
    merge reliably. Delete the read and whatever branches on it.
  - `coding_agent_has_diff`, `coding_agent_proposed`, `coding_agent_incomplete`
    and `coding_agent_requires_restart` → the one `coding_agent_change_state`
    object. Read its `kind`: `has_diff` is `kind != "none"`, and `proposed` is
    `kind == "proposed"`. `requires_restart` now lives on the `proposed`
    object. Unfinished work is `unproposed` with `reason: "turn_incomplete"`.
    Do NOT flag SQL that reads `thread_summaries.coding_agent_requires_restart`:
    that column still exists.

## Output

Write to `data/artifacts/audits/YYYY-MM-DD-HHMM/report.md` (user's local time; UTC if timezone unknown). Use `lucidos data write` so it lands in the workspace, not the worktree.

### Report structure

```markdown
# Workspace Audit: YYYY-MM-DD HH:MM

## Summary
- N findings across M categories
- Severity breakdown: <broken>/<stale>/<drift>/<smell>/<nit>
- Categories with no findings: <list>
- Targeted run only, in place of the line above: "Scope: <the one check>. Every
  other category is unexamined."

## Scan receipts
<the RECEIPTS table the scan printed, verbatim>

## <Category>
### <item> (<severity>)
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

The first two split on whether the thing still does its job, not on whether it
errors. Most findings are silent either way: a trigger on a retired name is
**broken** and never fires, while a trigger that lost its preloaded knowhow is
**stale** and fires without it.

### Event

```bash
lucidos events emit WorkspaceAuditCompleted \
  --summary "Workspace audit: <N> findings (<broken> broken, <stale> stale, <drift> drift, <smell> smell, <nit> nit)" \
  --payload '{"artifact": "artifacts/audits/<dir>/report.md", "findings": <N>, "broken": <A>, "stale": <B>, "drift": <C>, "smell": <D>, "nit": <E>}'
```

## Remediation: only on request, and only as child threads

Fixes are a separate step, only when the user asks: up front ("audit and fix what you can") or after reading the report ("go fix the app theming ones").

### Ask once, and lead with the whole set

The user usually answers all or none, so ask a **single-select** card before any multi-select one. A multi-select first costs one tap per batch for that answer, and its 4-option cap leaves no slot for "all" beside the batches.

Ask right after the report lands, with these options in this order:

| Option | Means |
|---|---|
| **Do all suggested fixes** | Every fix in the report. Say how many, across how many targets, in the description. |
| *A narrower cut* | Only when one exists and is genuinely safer or cheaper, such as the direct cleanups without the coding-agent work. Skip the slot otherwise. |
| **Let me pick which** | Follow with a multi-select card: one option per target batch, using the batching rule below. |
| **Nothing for now** | Leave the report as the record. It needs its own option: Submit stays disabled with nothing ticked, and Cancel aborts the turn rather than declining. |

The first label says *suggested* because each finding's own line says so. A label the report never uses makes the user hunt for its list.

**Skip the card when the user already asked for fixes.** "Audit and fix what you can" is the answer; re-asking bounces a settled decision back.

When you do spawn fix work:

- **Spawn child threads: omit `relation` (it defaults to `"child"`). Never pass `relation: "top"`.** When a child's session ends, this audit thread resumes with the result. You can then confirm each fix, note what didn't land, and update the report.
  - The fix threads nest under the audit in the thread drawer. Each one on a pending change is an *attention descendant*, which bubbles the audit thread to the Current section. The user follows one row, not N.
  - `relation: "top"` records no parent *and* no spawning event. Nothing links the fix to the audit, and the report stays frozen at "suggested fix". The `"top"` wording ("for the user to follow themselves") does **not** cover audit remediation.
  - Only exception: a fix in a *different* workspace must use `relation: "top"`. Child callbacks don't cross a workspace boundary, and the tool refuses the combination. Say so in the report and link the thread, since it won't report back.
- **One thread per target, spawned in parallel.** Issue the `run_coding_agent` calls in a single response; each reports back independently. Batch per app / per repo, not per finding: one thread fixing six findings in one `index.html` beats six threads racing on the file.
- **A child reporting back means its session ended, not that the fix is live.** Coding-agent work lands as a pending change the user applies. Report it as "proposed", never as "applied" or "live".
- **Fold the outcomes into the same report.** When the children have reported back, append a `## Remediation` section to the run's existing `report.md` (same timestamped directory). List target, spawned thread link, and outcome per fix. A fresh sweep gets a fresh directory; a remediation pass does not.

### Rewriting an app for an isolated frame

Hand a fix thread this table and the rules under it:

| Old | New |
|---|---|
| `localStorage.getItem('k')` / `setItem` / `removeItem` holding per-device state | the same call on `lucidos.storage.local`, after one `await lucidos.storage.ready` at startup |
| `sessionStorage.*` | the same call on `lucidos.storage.session`, after the same `await` |
| `localStorage` holding state every device should share | `await lucidos.data.read('artifacts/<app-id>/state.json')` and `lucidos.data.write(…)` |
| `indexedDB` or `caches` holding a large cache | `lucidos.data` under `artifacts/<app-id>/`. There is no in-frame replacement |
| `localStorage.getItem('lucidos-device-id')` | delete it. The host stamps the device on every bridged call, and the frame is not meant to know which one |
| an engine call the app's own `fetch` makes | the `lucidos.*` method that covers it, else `lucidos.request('/<suffix>', init)` |
| `fetch('<the app's own bundled file>')` | `await lucidos.data.read('apps/<app-id>/<file>')`, then parse. Not `lucidos.data.url()`, which builds a `src` and is refused when fetched |
| `lucidos.request('/env-vars', { method: 'POST' })` storing the app's own setting | `lucidos.preferences` for a user-facing one, else `lucidos.data.write('artifacts/<app-id>/settings.json', …)`. The read stays, so a genuine read of the workspace's variables is left alone |
| `lucidos.preferences.set` on a human-only key such as `command_guard` | nothing. It is a security setting, and the user changes it in Settings |
| `lucidos.preferences` reading or writing engine bookkeeping such as `vapid_keys` | nothing. It is engine state, not a setting: delete the read or the write |
| a call to a route the engine keeps from apps | nothing. Report it and name what it reaches for |
| `<a href="report.pdf" download>` on the app's own file | load `/api/v1/sdk.js`, which rewrites the click, or build a `blob:` URL |
| a `<base href>` the app declares | delete it. It replaces the pass the engine stamps for the app's own files |
| `history.replaceState(...)` / `history.pushState(...)` writing the app's own state into the URL | delete the write. Keep the read: apply `location.hash` at boot and on `hashchange` |
| `location.href = ...` / `location.assign(...)` navigating the frame itself | `lucidos.ui.navigate(...)` for a Lucidos destination, `lucidos.ui.openExternal(url)` for anything outside |

- **Await `lucidos.storage.ready` before the first read.** Reads are
  synchronous after it. A read before it sees nothing stored, which is the very
  bug being fixed. Move the startup read after the `await`, and drop the
  `try` / `catch` that hid the old `SecurityError`.
- **Pick the store by who should see the value.** `lucidos.storage` is per
  device, like `localStorage`. `lucidos.data` is one store for every device.
  A sound toggle or a display currency is per device. A list the user curates
  is usually shared. Say which you chose in the report.
- **No `window.localStorage` shim.** Rename the calls. Reference:
  `system-knowhow/js-sdk.md` § `lucidos.storage`.

### Opting an app into the ready signal

Hand a fix thread this table and the rule under it:

| Old | New |
|---|---|
| an app that renders empty until its startup data loads | add `"reveal": "on-ready"` to `manifest.json`, and call `lucidos.ui.ready()` after the first render on every path, error and empty included |
| `"reveal": "on-ready"` with no `lucidos.ui.ready()` call | add the call after the first render, or drop the opt-in |
| `reveal` set to anything else, such as `"onready"` | `"on-ready"` if the app calls `lucidos.ui.ready()`, else delete the field |

- **Find every path that ends the first render.** A fix tends to miss the
  empty state and the `catch` that shows an error, which then wait 15 s for
  the fuse.

### Giving an app its icon

| Old | New |
|---|---|
| `icon` that breaks `building-an-app.md` § App icon | fix the file, or draw a new icon as that section says |
| no `icon` | draw one as that section says |

### Replacing a control the app draws itself

Hand a fix thread this table and the rules under it:

| Old | New |
|---|---|
| `alert(message)` | `lucidos.ui.toast(message, 'error')` for a failure, else `'info'` |
| the app's own `toast(message)` and its element | `lucidos.ui.toast(message, type)`, then delete the element and its CSS |
| `if (confirm(message))` | `if (await lucidos.ui.confirm({ message }))`, with `danger: true` for a destructive action |
| `const value = prompt(message)` | `const value = await lucidos.ui.prompt({ message })`. It also returns `null` on cancel |
| a plain `<select>` | `lucidos.ui.enhanceSelects()` once the markup is on the page |
| `title="…"` as a tooltip | `data-tooltip="…"` |
| a switch the app draws itself | `<label class="toggle-switch"><input type="checkbox" role="switch"><span class="toggle-slider"></span></label>`, then delete the app's own switch CSS |
| a spinner the app draws itself | `<span class="mini-spinner" aria-hidden="true"></span>` beside its text, then delete the app's own spinner CSS and keyframes |
| a status or category chip the app draws itself | `<span class="label">`, plus `label-success`, `label-warning`, `label-error` or `label-neutral` for a status. Map the app's own colours onto those four and the bare accent, then delete its chip CSS |
| tabs the app draws itself | `<div class="pill-bar" role="tablist">` with a `<button class="pill-bar-btn" role="tab" aria-selected>` per view, then delete the app's own tab CSS |
| filter pills the app draws itself | the same `.pill-bar`, with `role="group"` and `aria-pressed` on each button |
| a text input or textarea the app draws itself | `<input class="text-input">` / `<textarea class="text-input">`, then delete the app's own border/background/focus CSS for it |

- **Confirm and prompt become asynchronous.** The SDK's return a promise, so
  the calling function becomes `async`, and every caller has to await it.
- **Keep the message and its language.** Pick the toast type from what the
  message says: success, a failure, or neutral information.
- **Every helper needs `sdk.js`.** An app whose `index.html` does not load
  `/api/v1/sdk.js` gets it first, per the boilerplate in `system-knowhow/js-sdk.md`
  § Setup.

### Rewriting an old-form `tap`

Hand a fix thread this mapping:

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
- **Flag a missing sibling, never guess one.** The old form let the engine fill the id in at write time; the new one does not. A navigate with no thread id raises the error toast `Navigation target missing thread id` on tap. Leave a `MIGRATION REVIEW` comment naming the two ways out: supply `to.id`, or fall back to `{ kind: 'modal' }`. List every such site in the report's `## Remediation` section.

### Asking plugins for an engine floor

| Old | New |
|---|---|
| Installed plugin has no `engine`; a compatible update exists | `plugins(action="update", id="<id>")`, which stages the confirm panel for the user to accept |
| Installed plugin has no `engine`; the available update needs a newer Lucidos | Tell the user to update Lucidos, quoting `engine_incompatible_reason`, then retry the plugin update |
| Installed plugin has no `engine`; no newer version declares one | Ask the plugin's author. Push the floor directly if `source` is a repo the user owns, else use `system-knowhow/plugins.md` § "Proposing your patch upstream" |
| Installed plugin has no `source` (archive install) | Ask whoever shared the plugin for an updated archive that declares `engine` |
| Authored plugin tree's `manifest.toml` has no `engine`, or an invalid one | Set `engine` to the plugin's floor release and bump `version`, in the author's own tree, never the installed copy |

- **Updates stage the confirm panel, never confirm themselves.** `plugins(action="update", ...)` opens the usual update panel. A fix thread or this audit turn stops there; the user clicks Confirm.
- **Never edit the installed copy** under `data/`, for the reason check 6 gives. Fix the plugin's own source tree, or propose the change upstream.

## Out of scope

- **No edits or deletes during the sweep.** Suggested fixes only; see § Remediation for the ask-first fix path.
- **No code-style linting.** That is `cargo fmt` / `prettier`.
- **No `.lucidos/` or `data/postgres/`** (ephemeral / event store, not user content).
- **No per-file artifact enumeration.** Structural rules only.
- **No codebase audit** (`crates/`, `cli/`, `scripts/`). This audits the workspace's *use* of those surfaces, not the surfaces themselves.

## Idempotency

Each run gets its own timestamped directory. Don't overwrite previous reports: diffing them shows whether drift is being addressed. On a same-minute collision, append a counter.

## Maintenance

A change to a referenced source-of-truth file (new SDK call, new convention, deprecation) can make a check stale. A check citing a heading or filename breaks silently when the source renames it. `.claude/rules/system-knowhow.md` § `Maintaining workspace-audit` says when this file must change with its sources. `./scripts/check-knowhow-refs.sh` catches the mechanical half in `/harden`.

**Several checks own their detection patterns outright**, rather than citing another file. Change what a check detects, and update its scan section, the check and its § Remediation table in the same change:

- **Old-form `tap`**: a change to the `Tap` type. The rule file above carries this as a row.
- **The isolated frame**: a change to the app frame's sandbox, or to what the app bridge carries. Its `fetch` half runs as two scan sections, `engine-fetch` (the engine address) and `relative-fetch` (the app's own files). One CORS refusal has two spellings in app code, so a sandbox change reaches both.
- **Hand-rolled controls**: the SDK or the shared component layer gains a control, so its hand-drawn form becomes drift. Also drop it from the "no SDK counterpart" list.
- **The ready signal**: the manifest's `reveal` values, the fuse lengths, or `lucidos.ui.ready()`.
- **The app icon**: what `icon` may name, the allowed file types, or the size cap.
- **Plugins**: the required manifest fields, the `engine` schema, or the nested event path `system-knowhow/plugins.md` § `PluginInstalled` documents. This check owns the `plugin-manifests` scan and its own reading of `PluginInstalled`.

**§ The mechanical scan is where a pattern RUNS, and a check is where it means
something.** Some checks also name their forms in words for the report and the
remediation: the five `tap` strings, the two removed CLI flags. Those are the
forms the block greps, so change both in the same edit. A new check with a
grep-able pattern needs a section in the block, or a pass can skip it unnoticed.

When a deprecated CLI flag or tool arg is fully removed, add it to check 8's "Removed CLI flags and tool args" list. Include its replacement and any live same-named flag to exclude. The source is `docs/temporary-measures.md` § sunset deprecations.
