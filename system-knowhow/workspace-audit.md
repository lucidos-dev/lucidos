---
name: Workspace Consistency Audit
description: Audits workspace apps, triggers, knowhow, intents, scripts, plugins, artifacts and prompt footprint against current conventions. Use to audit the workspace and check for drift. Finds stale or unused items, and moves apps off a retired pattern or onto SDK UI helpers.
---

# Workspace Consistency Audit

A read-only sweep that reports drift between what's on disk and current conventions. Output: one Markdown report under `data/artifacts/audits/` plus a `WorkspaceAuditCompleted` event.

The audit is split into *audit sections*, each a reference file under
`system-knowhow/workspace-audit/`. This root says how to run them and how to
report. Each section carries its own scan, checks and fixes.

## The audit sections

| id | load | covers |
|---|---|---|
| `triggers` | `system-knowhow/workspace-audit/triggers` | intent vs knowhow split, retired event subscriptions, slugs, notification taps |
| `apps` | `system-knowhow/workspace-audit/apps` | SDK boilerplate, the isolated app frame, escaping, theme names, hand-drawn controls, the ready signal, widgets |
| `knowhow` | `system-knowhow/workspace-audit/knowhow` | frontmatter, naming, placement, unreachable references, orphans |
| `intents` | `system-knowhow/workspace-audit/intents` | frontmatter, knowhow ids, tone |
| `scripts` | `system-knowhow/workspace-audit/scripts` | CLI usage, machine paths |
| `plugins` | `system-knowhow/workspace-audit/plugins` | the `engine` requirement |
| `artifacts` | `system-knowhow/workspace-audit/artifacts` | structural rules only |
| `cross-cutting` | `system-knowhow/workspace-audit/cross-cutting` | credentials in code, broken references, removed flags and fields |
| `prompt-footprint` | `system-knowhow/workspace-audit/prompt-footprint` | the *workspace prompt footprint*: sections over their ceilings, clipped descriptions, unused items |

Load a section with `load_knowhow` and its id, as for any knowhow.

## When to run this

User says "audit the workspace", "check for drift", "what's stale", "is everything still using the right pattern", "scan my apps/triggers", "is my prompt getting big". Or after a major SDK / CLI / system prompt change, where existing content might silently use the old shape.

The user's words pick one of three shapes:

- **A full pass**: an unqualified "audit my workspace". Run every section.
- **One audit section**: the user names a section ("run the prompt footprint
  audit", "audit my apps"). Run that section only, scan included.
- **A targeted run**: the user names one break. See below.

**Every full pass runs every section, with no delta pass.** Run every check
yourself, however recently the last one ran. Use a previous report to compare
*findings*, never to decide what to skip: a check added since is one it never
ran. Re-walking a surface the last run fixed is cheap, and confirms the fix held.

**Anything short of a full pass claims only what it ran.** Its report names
the sections it ran and carries no `Categories with no findings` line. A later
pass must not read it as coverage.

## A targeted run: one check, then fix it everywhere

Reach for a **targeted run** when the user already knows what broke: "migrate my
apps off `localStorage`", "fix the apps that call the engine themselves", "we
renamed X, sweep for it". It is one check plus its section's § Remediation, nothing else.

It differs from a full pass in four ways:

- **Pick the check by what the user named**, load the section that owns it,
  and run that one check. Where a check has sub-bullets, run the one that
  matches and say which.
- **Skip the inventory.** Walk only the surface that check names.
- **Go straight to the section's § Remediation.** The user asking to migrate has already
  answered "do the fixes", so do not ask again, and do not stop at a report.
- **Claim only what you ran.** The report carries the one check.

A targeted run is NOT a lighter audit. An unqualified "audit my workspace" is
always the full pass.

**Read-only.** Never edit or delete during the audit; the report proposes fixes.
This covers *every* mutation: no `rmdir`/`rm`, no writing a `.gitignore`, no
`git add`/`git commit`, no `run_coding_agent`. If you catch yourself mutating
mid-sweep, put the fix in the report instead. Fixes follow § Remediation below.

## Sources of truth: load these first

The audit does **not** restate the rules. Each check names the file that owns
its rule; load it for the canonical wording.

| Reference | Owns |
|---|---|
| `system-knowhow/best-practices.md` | Workspace file conventions: artifacts/, apps/, knowhow/, intents/, scripts/, config/ layout, per-workspace environment variables (Settings → System → Environment variables / the grouped `env_vars` tool), naming, "never nest artifacts", import-the-minimum |
| `system-knowhow/js-sdk.md` | Current app HTML boilerplate and the full `lucidos.*` API surface (anything not listed is either deprecated or invented) |
| `system-knowhow/lucidos-cli.md` | What scripts and coding-agent subprocesses use for `data.*` writes, `events.*` emits, `proxy` calls to external APIs (preferred over raw `curl -H "Authorization: ..."` with `$CRED_*`), and `spawn-thread` thread spawning (sub-threads + cross-workspace, including Codex via `--codex`) |
| `system-knowhow/plugins.md` | The plugin manifest schema, the `engine` requirement, and `check_plugin_updates` / `update_plugin` semantics |
| `system-knowhow/building-knowhow.md` | Knowhow doc vs reference: which files a root lists, and where a doc's own supporting files go |
| `system-knowhow/intent-registry.md` | Which on-disk files become intents in the system prompt (trigger files double as intents, which is easy to miss) |
| `system-knowhow/thread-events.md` | Every `ThreadEvent` name and which of them a trigger can subscribe to. For the **retired** set, ask the `events` tool rather than reading this file: see the `triggers` section |
| The active engine system prompt | The intent vs knowhow taxonomy, the trigger worked example |

"Per `<file>`" means: read the current version of that file and use *its*
wording, not your memory of it.

**Reach every one of them, this file and its sections, with `load_knowhow`.** Never look for
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
| The prompt footprint | `lucidos workspace-prompt-footprint show`, not a `data/` path |

Trigger intent text lives in the `TriggerCreated` event payload (`run.intent`), not on disk. Pull it with `lucidos events query --type TriggerCreated`.

## The mechanical scan: run this first, verbatim

A grep decides every pattern below, not your judgment. Run the whole scan as ONE
call, before reading any file, so coverage is a command that ran or did not.
From memory, a check gets skipped, and a skipped check reads like a clean one.

`run_bash` starts in the workspace root, so this needs no absolute path.

Build the call from three parts: the preamble below, then each section's
`## Scan` lines, then the tail below. A full pass takes every section's lines.
One audit section takes its own. The `prompt-footprint` section's scan is a
CLI call that runs on its own, and adds its own receipts row.

Each scan prints its hit count, and the tail repeats them as one table. That
table is the **receipt**: proof the scan ran, and the only honest basis for
calling a category clean.

| scan | what it looks for | the section that judges it |
|---|---|---|
| `storage` | browser storage an app frame cannot reach | `apps` |
| `host-realm` | the shell, read from an app frame | `apps` |
| `engine-fetch` | the app calling the engine itself, `apiUrl` included | `apps` |
| `relative-fetch` | the app fetching its own bundled file by relative path | `apps` |
| `download-link` | a download link, which needs `sdk.js` in a frame | `apps` |
| `media-capture` | the camera or the microphone, from an app frame | `apps` |
| `web-share` | the OS share sheet, from an app frame | `apps` |
| `url-mutation` | the frame writing its own session-history URL | `apps` |
| `theme-rename` | names from before *look* became *theme* | `apps` |
| `hand-rolled-ui` | a control the app draws itself that the SDK provides | `apps` |
| `attr-escape` | a text-only escaper writing into an attribute value | `apps` |
| `ready-signal` | the manifest's `reveal`, and calls to `lucidos.ui.ready()` | `apps` |
| `app-icon` | the manifest's `icon` | `apps` |
| `touch-hover` | every `:hover` rule, to judge whether a hover gate wraps it | `apps` |
| `state-as-action` | `.action-btn` variants swapped in code, or written without the base class | `apps` |
| `tap-strings` | the retired `tap` string forms | `triggers` |
| `removed-flags` | CLI flags and tool args that were removed | `cross-cutting` |
| `removed-fields` | thread-summary fields the engine no longer sends | `cross-cutting` |
| `cred-env` | a credential read from the environment | `cross-cutting` |
| `auth-header` | an auth header built in app UI code | `cross-cutting` |
| `machine-path` | a hardcoded home or machine path | `scripts` |
| `plugin-manifests` | authored plugin-root `manifest.toml` files and whether each declares `engine` | `plugins` |
| `prompt-footprint` | the sections the footprint report lists | `prompt-footprint` |

The preamble:

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
```

The tail:

```bash
printf '\n=== RECEIPTS: copy this table into the report ===\n'
printf '| section | hits |\n|---|---|\n'
printf '%b' "$receipts"
```

**A hit is evidence, not a finding.** The owning section's check says what a hit means,
how bad it is, and what to recommend. Some hits are fine in context: `apiUrl`
building a `src` is correct, and a `try` around a storage call changes the
severity without clearing it. Report the hits that stand.

**The receipts table goes in the report, verbatim.** A `0` is a result: the
category is clean. A scan MISSING from the table is a category nobody looked
at. Say so in the report rather than dropping the row, because a reader counts an
absent row as clean.

## Output

Write to `data/artifacts/audits/YYYY-MM-DD-HHMM/report.md` (user's local time; UTC if timezone unknown). Use `lucidos data write` so it lands in the workspace, not the worktree.

### Report structure

```markdown
# Workspace Audit: YYYY-MM-DD HH:MM

## Summary
- N findings across M categories
- Severity breakdown: <broken>/<stale>/<drift>/<smell>/<nit>
- Categories with no findings: <list>
- Short of a full pass, in place of the line above: "Scope: <the sections, or
  the one check>. Every other category is unexamined."

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

Each section's own `## Remediation` carries the tables a fix thread gets: the
old-to-new mappings, and which fixes may join "Do all suggested fixes".

- **A deletion never joins "Do all suggested fixes".** Deleting an app or a
  knowhow doc gets its own card, naming the item.
- **Never raise a ceiling or change a preference to clear a finding.** Only the
  user's own words may move one.

## Out of scope

- **No edits or deletes during the sweep.** Suggested fixes only; see § Remediation for the ask-first fix path.
- **No code-style linting.** That is `cargo fmt` / `prettier`.
- **No `.lucidos/` or `data/postgres/`** (ephemeral / event store, not user content).
- **No per-file artifact enumeration.** Structural rules only.
- **No codebase audit** (`crates/`, `cli/`, `scripts/`). This audits the workspace's *use* of those surfaces, not the surfaces themselves.

## Idempotency

Each run gets its own timestamped directory. Don't overwrite previous reports: diffing them shows whether drift is being addressed. On a same-minute collision, append a counter.

## Maintenance

A change to a referenced source-of-truth file (new SDK call, new convention, deprecation) can make a check stale. Every check lives in one section file, so update that file. A check citing a heading or filename breaks silently when the source renames it. `.claude/rules/system-knowhow.md` § `Maintaining workspace-audit` says when this file must change with its sources. `./scripts/check-knowhow-refs.sh` catches the mechanical half in `/harden`.

**Several checks own their detection patterns outright**, rather than citing another file. Change what a check detects, and update its section's scan lines, the check and its § Remediation table in the same change:

- **Old-form `tap`**: a change to the `Tap` type. The rule file above carries this as a row.
- **The isolated frame**: a change to the app frame's sandbox, or to what the app bridge carries. Its `fetch` half runs as two scan sections, `engine-fetch` (the engine address) and `relative-fetch` (the app's own files). One CORS refusal has two spellings in app code, so a sandbox change reaches both.
- **Hand-rolled controls**: the SDK or the shared component layer gains a control, so its hand-drawn form becomes drift. Also drop it from the "no SDK counterpart" list.
- **The ready signal**: the manifest's `reveal` values, the fuse lengths, or `lucidos.ui.ready()`.
- **The app icon**: what `icon` may name, the allowed file types, or the size cap.
- **Plugins**: the required manifest fields, the `engine` schema, or the nested event path `system-knowhow/plugins.md` § `PluginInstalled` documents. This check owns the `plugin-manifests` scan and its own reading of `PluginInstalled`.

**A section's `## Scan` is where a pattern RUNS, and its check is where it
means something.** Some checks also name their forms in words for the report and the
remediation: the five `tap` strings, the two removed CLI flags. Those are the
forms the block greps, so change both in the same edit. A new check with a
grep-able pattern needs scan lines in its section and a row in the receipts
table above, or a pass can skip it unnoticed.

When a deprecated CLI flag or tool arg is fully removed, add it to the `cross-cutting` section's "Removed CLI flags and tool args" list. Include its replacement and any live same-named flag to exclude. The source is `docs/temporary-measures.md` § sunset deprecations.

**A new section** is a file under `system-knowhow/workspace-audit/`, a row in
§ The audit sections, and its scans in the receipts table. Name it by id there:
a nested file is in no routing list, so this root is the only way to reach it.
