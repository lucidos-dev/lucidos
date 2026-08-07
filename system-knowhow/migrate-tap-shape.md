---
name: Migrate Notification Tap Shape
description: Recipe for rewriting old-form `tap` strings ('modal' | 'open_app' | 'open_thread' | 'none') in a workspace's triggers and apps to the structured `Tap` object ({kind, to?}). Use when the workspace audit flags old-form taps.
---

# Migrate Notification Tap Shape

The notification `tap` field changed from a four-string union to a discriminated union object. Old workspaces have triggers and apps that still POST `/api/v1/notifications` with `"tap": "modal"` (or call `lucidos notify --tap open_thread`). The new engine rejects those calls with 400 at write time. This recipe rewrites them in place, per-workspace.

## When to run this

- The workspace audit (`system-knowhow/workspace-audit.md`) flagged old-form tap usage.
- A trigger fired and the engine returned `400 Bad Request` on `notifications.create` with the error mentioning `tap`.
- After upgrading the engine to a version that hard-rejects old-form tap.

**This recipe edits files.** Unlike the workspace audit (which is read-only), this one rewrites trigger scripts and app code. Run it once per workspace.

## The two shapes

| Old (string) | New (object) |
|---|---|
| `tap: 'modal'` | `tap: { kind: 'modal' }` |
| `tap: 'none'` | `tap: { kind: 'modal' }` (the passive `none` kind was retired — every notification is openable; `docs/plans/2026-07-02-remove-notification-tap-none.md`) |
| `tap: 'open_app'` (with sibling `app_id: 'X'` on the same call) | `tap: { kind: 'navigate', to: { target: 'app', app_id: 'X' } }` |
| `tap: 'open_thread'` (with sibling `thread_id: 'T'`, optional `event_id: 'E'`) | `tap: { kind: 'navigate', to: { target: 'thread', id: 'T', event_id: 'E' } }` |

For `open_app` and `open_thread`, the new `to.app_id` / `to.id` / `to.event_id` come from the same call's sibling fields (`app_id`, `thread_id`, `event_id`). When sibling fields aren't present at the call site (they were going to be inferred from the notification context at engine time), produce the navigate shape with empty sub-fields and flag the call for manual review — the author originally relied on the engine to fill them in, and the new shape makes that explicit.

See `system-knowhow/js-sdk.md` § `lucidos.notifications` for the canonical Tap type.

## Where to walk

Per the workspace's `data/` layout (resolve via the `lucidos` CLI — `lucidos data ls`):

| Surface | Path | Languages to scan |
|---|---|---|
| Standalone triggers | `data/triggers/<slug>/scripts/` | `.py`, `.sh`, `.js`, `.ts` |
| App-scoped triggers | `data/apps/<id>/triggers/<slug>/scripts/` | `.py`, `.sh`, `.js`, `.ts` |
| Apps (custom UI code calling the SDK) | `data/apps/<id>/ui/`, `data/apps/<id>/*.html`, `data/apps/<id>/*.{js,ts}` | `.js`, `.ts`, `.html`, inline `<script>` |
| Shared scripts | `data/scripts/` | `.py`, `.sh`, `.js`, `.ts` |
| Knowhow with embedded examples | `data/knowhow/**/*.md` (fenced code blocks) | check `python`, `bash`, `js`, `ts` fences |

Skip `data/artifacts/` (user content; no executable code that calls the SDK) and `data/postgres/` (event store; not user-editable).

## Detection patterns

Grep each file for these patterns. The list is the same one the audit's `tap_shape` check uses — keep them in sync if you edit one.

### Python / shell / generic-string

- `"tap": "modal"` / `'tap': 'modal'`
- `"tap": "none"` / `'tap': 'none'`
- `"tap": "open_app"` / `'tap': 'open_app'`
- `"tap": "open_thread"` / `'tap': 'open_thread'`

### JavaScript / TypeScript

- `tap: 'modal'` / `tap: "modal"`
- `tap: 'none'` / `tap: "none"`
- `tap: 'open_app'` / `tap: "open_app"`
- `tap: 'open_thread'` / `tap: "open_thread"`
- `'tap': 'modal'` (key-quoted variants — less common but exists)

### URL-encoded / hash-form

Don't rewrite these — the URL channel of the SW is a runtime concern, not a workspace-file concern. The engine + SW are the source of truth there.

## Rewrite mechanics

For each match, build the new shape from the call's sibling fields:

App code creating notifications calls the engine HTTP API directly — `lucidos.notifications.*` only exposes `list` / `markRead` / `markAllRead`. Rewrite the body the call POSTs:

```js
// Before
await fetch('/api/v1/notifications', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    title: 'Heads up',
    message: 'Pick one',
    app_id: 'habit-tracker',
    tap: 'open_app',
  }),
});

// After
await fetch('/api/v1/notifications', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    title: 'Heads up',
    message: 'Pick one',
    app_id: 'habit-tracker',
    tap: { kind: 'navigate', to: { target: 'app', app_id: 'habit-tracker' } },
  }),
});
```

```python
# Before
requests.post(NOTIFY_URL, json={
    "title": "5 changes ready",
    "message": "Review changes panel",
    "tap": "open_thread",
    "thread_id": str(thread_id),
    "event_id": str(event_id),
})

# After
requests.post(NOTIFY_URL, json={
    "title": "5 changes ready",
    "message": "Review changes panel",
    "tap": {
        "kind": "navigate",
        "to": {
            "target": "thread",
            "id": str(thread_id),
            "event_id": str(event_id),
        },
    },
    "thread_id": str(thread_id),
    "event_id": str(event_id),
})
```

(Keep the sibling `thread_id` / `app_id` / `event_id` on the call — they're notification-level context, used by the §4 in-app matrix and the inbox modal even when the tap navigates elsewhere.)

When the call is `tap: 'open_app'` or `'open_thread'` and the sibling field is missing or computed dynamically (e.g. `app_id` comes from a function call), apply the rewrite by reusing the same expression:

```python
# Before
notif = build_notif(app_id=resolve_app(), tap="open_app")

# After
app = resolve_app()
notif = build_notif(
    app_id=app,
    tap={"kind": "navigate", "to": {"target": "app", "app_id": app}},
)
```

When the sibling field isn't available at the call site, the legacy behavior relied on engine-side inference. Translate to a navigate-with-no-payload AND leave a comment for review:

```js
// Before
sendNotif({ tap: 'open_thread' });  // relied on engine-injected thread_id

// After — MIGRATION REVIEW: the new shape needs the thread id at the call
// site (the engine no longer injects it). Pick one of:
//   (a) supply `to.id` explicitly,
//   (b) drop the navigate and fall back to modal (`{ kind: 'modal' }`).
// A navigate to an unknown target now surfaces "Navigation target missing
// thread id" as an error toast on tap rather than silently no-op'ing —
// fix-or-fall-back is required.
sendNotif({
  tap: { kind: 'navigate', to: { target: 'thread', id: '<set me>' } },
});
```

Report these `MIGRATION REVIEW` sites in the recipe's output so the user can audit them.

## Output

After all rewrites, write a summary to `data/artifacts/migrations/YYYY-MM-DD-HHMM-tap-shape/report.md` (user's local time; UTC if timezone unknown):

```markdown
# Tap-shape Migration — YYYY-MM-DD HH:MM

## Summary
- Files scanned: N
- Files modified: M
- Replacements: K
- Sites flagged for manual review: J

## Modified files

### data/triggers/<slug>/scripts/run.py
- Line 42: `tap="open_thread"` → `tap={"kind":"navigate","to":{"target":"thread","id":str(thread_id),"event_id":str(event_id)}}`

### data/apps/<id>/ui/index.html
- Line 87: `tap: 'open_app'` → `tap: { kind: 'navigate', to: { target: 'app', app_id: 'habit-tracker' } }`

## Sites flagged for manual review

### data/triggers/<slug>/scripts/run.py:67
- Original: `sendNotif({ tap: 'open_thread' })` — relied on engine-injected thread_id
- Action: review whether the caller can now supply `to.id` directly, or fall back to modal.
```

Use `lucidos data write` (not the worktree filesystem) so the report lands in the workspace.

## Event

```bash
lucidos events emit TapShapeMigrationCompleted \
  --summary "Migrated tap shape: <M> files, <K> replacements, <J> needs-review" \
  --payload '{"artifact": "artifacts/migrations/<dir>/report.md", "files_modified": <M>, "replacements": <K>, "needs_review": <J>}'
```

## Idempotency

Each run gets its own timestamped directory. Running twice is safe — the second pass finds no old-form taps to rewrite and produces an empty report.

## Out of scope

- **Notification rows in the projection table** — the engine's SQL migration (`20260522152123_notification_tap_jsonb.sql`) handles those automatically on startup. Don't try to rewrite `notifications.tap` from this recipe.
- **Historical event payloads** — events are immutable per the Lucidos event-sourcing rule, and the new `Tap` deserializer is strict (rejects bare strings). Normal operation never re-deserializes the old strings — the projection was built once by the JSONB migration and incremental updates only consume new events with the canonical shape. The only path that would touch them is a force projection rebuild, which will fail loudly on a pre-migration `NotificationCreated` event and prompt the workspace owner to either accept the loss (legacy notifications drop from the projection) or hand-migrate the events table. Don't touch the events table proactively from this recipe.
- **URL deep-link parsing / SW push payloads** — runtime concerns owned by the engine + service worker. Updating engine + frontend is enough.
- **Codebase audit** (`crates/`, `cli/`, `scripts/`) — this is per-workspace user content only.

## Maintenance

When the `Tap` shape changes again (new kind added, structure shifts), update the detection patterns and the rewrite mechanics here in the same commit as the type change. See `.claude/rules/system-knowhow.md` for the drift-prevention rule.
