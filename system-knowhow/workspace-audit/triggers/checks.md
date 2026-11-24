---
name: Workspace audit: triggers, Remediation
description: Part of Workspace audit: triggers. Its index lists the sections it covers.
---

One part of the `system-knowhow/workspace-audit/triggers` audit section. Load every part of the section to run it; the section file lists them.

## Checks (continued)

  ```bash
  psql "$LUCIDOS_DATABASE_URL" -At -F ' ' -c "WITH RECURSIVE origin AS (SELECT DISTINCT ON (thread_id) thread_id, payload->'origin' AS o, (payload->>'parent_thread_id')::uuid AS linkage FROM events WHERE event_type IN ('TriggerStarted', 'MessageReceived') AND payload ? 'origin' AND payload->'origin' != 'null'::jsonb ORDER BY thread_id, sequence), edge AS (SELECT thread_id, CASE o->>'kind' WHEN 'thread_link' THEN CASE WHEN o->>'direction' = 'parent' THEN coalesce(linkage, (o->>'thread_id')::uuid) END WHEN 'workspace' THEN (o->>'thread_id')::uuid WHEN 'api' THEN CASE WHEN o->>'mode' <> 'human' THEN (o->>'source_thread_id')::uuid END END AS caused_by FROM origin UNION SELECT thread_id, (payload->'origin'->>'submitted_by_thread_id')::uuid FROM events WHERE event_type IN ('MessageReceived', 'PromptInjected') AND payload->'origin'->>'kind' = 'saved_draft' UNION SELECT thread_id, parent_thread_id FROM thread_summaries WHERE parent_thread_id IS NOT NULL), fire AS (SELECT thread_id, o->'reason'->>'trigger_id' AS trigger_id FROM origin WHERE o->'reason'->>'kind' = 'scheduler' UNION SELECT thread_id, o->>'emitting_trigger_id' FROM origin WHERE o->>'kind' = 'api' AND o ? 'emitting_trigger_id' UNION SELECT thread_id, payload->'origin'->'reason'->>'trigger_id' FROM events WHERE event_type = 'MessageReceived' AND payload->'origin'->'reason'->>'kind' = 'scheduler' UNION SELECT thread_id, trigger_id FROM thread_summaries WHERE trigger_id IS NOT NULL), tree AS (SELECT thread_id, trigger_id, 0 AS depth FROM fire UNION SELECT e.thread_id, t.trigger_id, t.depth + 1 FROM edge e JOIN tree t ON e.caused_by = t.thread_id WHERE t.depth < 16) SELECT t.trigger_id, count(DISTINCT t.thread_id), max(s.last_activity) FROM tree t JOIN thread_summaries s USING (thread_id) WHERE s.is_coding_agent GROUP BY 1"
  ```

  Keep each row whose trigger is live with `coding_agent_grant` false in `lucidos triggers list`. For each one, read its `run.intent`, script and knowhow, and say what steers it. Name every outside source it reads, such as email, chat messages, web pages, issue trackers or webhooks, since that content steers its coding agents too. Link the trigger as `[Name](trigger:<id>)`, and tell the user to tick *Allow coding agents* there. Severity: **broken**, since each such run is now refused, with a notification.

- **A trigger whose recent runs performed a side effect outside its grant.** With the command guard on, irreversible side effects require a matching grant. Check the trigger's approved program scopes (`system-knowhow/triggers/grouping-triggers.md` § "Program grants"). The guard now defaults on, so a trigger that ran unguarded may fail its next run. Find commands with recorded successful tool results over the last 30 days:

  ```bash
  psql "$LUCIDOS_DATABASE_URL" -At -F ' ' -c "
    SELECT fire.payload->>'trigger_id', call.payload->>'name', left(call.payload->>'args', 400)
    FROM events fire
    JOIN events call ON call.thread_id = fire.thread_id AND call.event_type = 'ToolCalled'
      AND call.payload->>'request_event_id' = fire.id::text
    WHERE fire.event_type = 'TriggerStarted' AND call.created >= now() - interval '30 days'
      AND call.payload->>'name' IN ('run_bash', 'run_bash_background', 'run_python', 'run_python_background')
      AND EXISTS (
        SELECT FROM events result WHERE result.thread_id = call.thread_id
          AND result.event_type = 'ToolResult' AND result.payload->'success' = 'true'::jsonb
          AND result.payload->>'tool_called_event_id' = call.id::text
      )"
  ```

  Review the command against that trigger's `program_grants` in `lucidos triggers list`. Report the trigger's link as `[Name](trigger:<id>)`, the uncovered program and one example command. Severity: **broken** while the guard is on, since the next such run fails with a notification. Skip the check when the workspace turned the guard off. A failed run's text ends with a `Decision trace:` line, which names the layer that gated the command.

- **A trigger script that reaches the workspace database through libpq defaults.** The `scripts` section owns the check and its fix (`system-knowhow/workspace-audit/scripts.md`, the `db-defaults` scan). Its scan covers `data/triggers/**/scripts/**` too. Report those hits once, under the trigger. Severity: **broken**, since each fire now fails to connect.

- **A trigger relying on knowhow a plugin removed, with no warning on record.** An update or uninstall now records its *affected triggers* and warns each fire (`system-knowhow/plugins/three-distribution-shapes.md` § "Affected triggers"). Two cases escape it: a removal from before that, and a trigger made after the removal. Find them by hand:

  ```bash
  psql "$LUCIDOS_DATABASE_URL" -At -F ' ' -c "SELECT e.event_type, e.aggregate_id, f FROM events e, jsonb_array_elements_text(coalesce(e.payload->'data'->'files_removed', e.payload->'data'->'files_deleted', '[]')) f WHERE e.event_type IN ('PluginInstalled', 'PluginUninstalled') AND e.payload->'data'->'affected_triggers' IS NULL"
  ```

  Keep each path still missing under `data/`. For each live trigger, report a kept path its `run.intent` or script path names, or knowhow under its own `apps/<app_id>/knowhow/` or `triggers/<slug>/knowhow/`. Skip triggers that plugin owns. Severity: **stale**, since each fire runs without its recipe. Fix per `system-knowhow/triggers/grouping-triggers.md` § "Know-how a plugin removed".

- **Old-form `tap` strings.** The field was a four-string union and is now a discriminated union object. The engine rejects the strings with `400 Bad Request` at write time. Grep every code-bearing surface in the root's § "What to walk": trigger scripts, app code (`ui/`, `*.html`, inline `<script>`), shared scripts, and fenced `python` / `bash` / `js` / `ts` blocks in knowhow. Skip `data/artifacts/` and `data/postgres/`, which hold no code that calls the API.

  Match each form whatever its quotes. The key is bare (`tap:` in JS, TS) or quoted (`"tap":` or `'tap':` in Python, shell, JSON bodies). The value takes either quote, and JSON inside a string escapes both (`\"tap\": \"modal\"`):

  - `tap: 'modal'`
  - `tap: 'none'`
  - `tap: 'open_app'`
  - `tap: 'open_thread'`
  - `{ kind: 'none' }`, the retired object form
  - `lucidos notify --tap none`, `--tap open-app` or `--tap open-thread`, bare or in an argv list. The CLI now takes only `modal` and `navigate`.

  A `kind: 'none'` hit outside a notification `tap` is some other field, not this finding.

  Surface path, line and the matched form. Severity: **broken**, because the next fire 400s. The one exception is the retired `{ kind: 'none' }` object. The engine coerces it to `{ kind: 'modal' }`, so it is **stale**: it runs, but still spreads by copy. Canonical `Tap` type: `system-knowhow/js-sdk/lucidos-preferences-user-settings.md` § `lucidos.notifications`.

  URL-encoded and hash-form taps are **out of scope**. The engine and the service worker own that channel, not any workspace file.

  § Remediation below carries the old-to-new mapping a fix needs.

## Remediation

For widget-fit recipes, require an open eligible window or an explicitly selected fit-only renderer (`docs/widget-fit-renderer.md`).
A trigger's source-thread origin may register a renderer; it cannot supply a browser or owner authority.
Flag recipes treating a timeout or `not checked` as success as **broken**.
`WidgetFitRequested` remains transient and cannot trigger another run.

Fixes run only on request, as the root's § Remediation says. A fix gets the
table for its finding, and the root's § Inline or a coding agent picks its
route.

### Listing an MCP server a trigger uses

**Route: inline**, one `triggers(action="update")` call. Add the server to the trigger's `mcp_servers`. The field is a full replacement, so send the existing list plus the new id. It is reversible, so it may join "Do all suggested fixes". A server the fire enabled once and never called is not worth listing: report it, and leave the choice to the user.

### Granting a trigger coding agents or a program

**Route: the user only.** Report each affected trigger, what steers it and the grant it needs.
For a folder grant, link the blocked run's permission request, or ask ahead with `grant-trigger-folders`, and state the folder.
For coding-agent access, ask for it in a batch if the user requests one.

For program grants from earlier runs, point the owner to **Check my workspace** in the release notice.
The review is a Lucidos Agent thread that asks one trigger per card.
Each card saves its grants without running the trigger.

A blocked intent run's permission request offers Allow once or Always allow, and the run carries on. A blocked script run's request grants and reruns. Both also offer Always deny.
A program or folder the trigger's run ended on with no card may be a *trigger deny* (`system-knowhow/triggers/grouping-triggers.md` § Trigger denies).
Report it, and leave lifting the deny to the owner in the trigger's details.
If the result says the guard could not rule out workspace deletion, cite
`system-knowhow/running-python/pick-the-right-tool.md` § The command guard (dangerous commands are gated).

Recommend an owner-started coding agent where outside content steers the trigger.

### Rewriting an old-form `tap`

**Route: a coding agent, one per trigger.** The rewrite builds new fields from computed siblings and flags the sites it cannot fill, which takes judgment. Hand it this mapping:

| Old | New |
|---|---|
| `tap: 'modal'` | `tap: { kind: 'modal' }` |
| `tap: 'none'` | `tap: { kind: 'modal' }`, since the passive kind was retired and every notification is openable |
| `tap: 'open_app'`, with a sibling `app_id: 'X'` | `tap: { kind: 'navigate', to: { target: 'app', app_id: 'X' } }` |
| `tap: 'open_thread'`, with a sibling `thread_id: 'T'` and optional `event_id: 'E'` | `tap: { kind: 'navigate', to: { target: 'thread', id: 'T', event_id: 'E' } }` |
| `{ kind: 'none' }` | `{ kind: 'modal' }` |
| `--tap none` | drop the flag, since `modal` is the default |
| `--tap open-app` or `--tap open-thread` | `--tap navigate`, keeping the call's `--app-id` or `--thread-id` |

Three rules the rewrite has to follow:

- **Build the new sub-fields from the call's own sibling fields, and keep those siblings.** They are notification-level context. The §4 in-app matrix and the inbox modal both read them, even when the tap navigates elsewhere.
- **Reuse the expression when a sibling is computed.** Hoist the app-resolution call into a local, then pass that local to both `app_id` and `to.app_id`. Calling it twice risks two different answers.
- **Flag a missing sibling, never guess one.** The old form let the engine fill the id in at write time; the new one does not. A navigate with no thread id raises the error toast `Navigation target missing thread id` on tap. Leave a `MIGRATION REVIEW` comment naming the two ways out: supply `to.id`, or fall back to `{ kind: 'modal' }`. List every such site in the report's `## Remediation` section.

## Diagnostic evidence

Run `lucidos diagnostic-log query` to investigate a trigger fire.
See [Agent Sandbox](../../agent-sandbox.md) for the access rules.
A maintainer bundle omits legacy log text; its absence does not establish an audit finding.

When investigating a routine run in Review, check the recorded `ThreadReadRequested` event.
A trigger agent's `request_read` yes only asks the turn-end judge to decide.
The `go_to_review` setting controls whether the completed run lands in Current.
