---
name: Workspace audit: triggers
description: The triggers audit section: procedure in intent, stale run.knowhow, subscriptions on retired event names, slugs, orphaned trigger knowhow, notification taps.
---

# Workspace audit section: triggers

One *audit section* of `system-knowhow/workspace-audit`. The root says how to
run it alone or in a full pass, how to merge its receipts, and how to write
the report. This file carries its scan, its checks and its fixes.

## Scan

Append these lines to the root's scan preamble, before its receipts tail:

```bash
scan tap-strings "\"tap\"[[:space:]]*:[[:space:]]*\"(modal|none|open_app|open_thread)\"|tap:[[:space:]]*'(modal|none|open_app|open_thread)'|kind:[[:space:]]*'none'" $all
```

## Checks

For each finding, capture: **location**, **what's wrong**, **which reference owns the rule** (link, don't quote), **suggested fix**.

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

- **Old-form `tap` strings.** The field was a four-string union and is now a discriminated union object. The engine rejects the strings with `400 Bad Request` at write time. Grep every code-bearing surface in the root's § "What to walk": trigger scripts, app code (`ui/`, `*.html`, inline `<script>`), shared scripts, and fenced `python` / `bash` / `js` / `ts` blocks in knowhow. Skip `data/artifacts/` and `data/postgres/`, which hold no code that calls the API.

  Match each form in both quote styles: the key-quoted `"tap":` spelling (Python, shell, JSON bodies) and the bare `tap:` spelling (JS, TS):

  - `tap: 'modal'`
  - `tap: 'none'`
  - `tap: 'open_app'`
  - `tap: 'open_thread'`
  - `{ kind: 'none' }`, the retired object form

  Surface path, line and the matched form. Severity: **broken**, because the next fire 400s. The one exception is the retired `{ kind: 'none' }` object. The engine coerces it to `{ kind: 'modal' }`, so it is **stale**: it runs, but still spreads by copy. Canonical `Tap` type: `system-knowhow/js-sdk.md` § `lucidos.notifications`.

  URL-encoded and hash-form taps are **out of scope**. The engine and the service worker own that channel, not any workspace file.

  § Remediation below carries the old-to-new mapping a fix thread needs.

## Remediation

Fixes run only on request, as the root's § Remediation says. A fix thread gets
the table for its finding.

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
