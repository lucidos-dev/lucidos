---
name: Workspace audit: triggers, Scan
description: Part of Workspace audit: triggers. Its index lists the sections it covers.
---

One part of the `system-knowhow/workspace-audit/triggers` audit section. Load every part of the section to run it; the section file lists them.

## Scan

Append these lines to the root's scan preamble, before its receipts tail:

```bash
scan tap-strings "[\\\"']*tap[\\\"']*[[:space:]]*:[[:space:]]*[\\\"']+(modal|none|open_app|open_thread)[\\\"']|kind[\\\"']*[[:space:]]*:[[:space:]]*[\\\"']+none[\\\"']|--tap[=[:space:]\\\"',]+(none|open[-_]app|open[-_]thread)" $all
```

## Checks

- **A handoff treated as landing.** `ChangeHandoffStarted` records the start of publication. Use `ChangeApplied` to react to a source change landing.
- **Trigger grants treated as exceptions to standing denies.** A standing
  deny takes precedence over the trigger's command permissions. Flag
  recipes that promise otherwise as **broken**.

- **A held build mistaken for failed Apply.** Check follow-up recipes against `system-knowhow/lucidos-cli/subcommands-10.md` § `lucidos changes apply <change-id>`.
  Flag triggers retrying an applied change only because its new version waits as **broken**.

- **Apps routing trigger management through another gateway.** Workspace routing endpoints refuse app bridge calls.
  Flag such recipes as **broken**; see `system-knowhow/js-sdk/lucidos-apiurl.md` § Not every endpoint is reachable.


- **Claimed caller threads treated as authority.** An `unattested_caller` may carry `claimed_thread_id` as a display hint.
  Flag filters granting authority from it as **broken**; see `system-knowhow/thread-events/wire-format-and-metadata.md` § Another workspace.

- **Take-backs counted as delivery.** A device Stop emits `QueuedMessageRemoved` for pending human inputs that no recipient has consumed.
  Check recipes against `system-knowhow/thread-events.md` before treating acceptance or removal as proof of delivery.
  Automation cancellation leaves these inputs alone. Stop restoration is durable compose state in the same removal transaction, not a second user submission.

- **Host-log consent treated as a trigger grant.** Check transfer recipes against `system-knowhow/lucidos-cli/subcommands-2.md` § `lucidos diagnostic-log prepare-host-transfer`.
  Flag recipes promising unattended transfers or reusing an earlier approval as **broken**.
  Apps cannot invoke these routes; see `system-knowhow/js-sdk/lucidos-apiurl.md` § Not every endpoint is reachable.

- **SDK bridge operations mistaken for event subscriptions.** `media.poster` is an app request, not a triggerable event.
  Check subscriptions against `system-knowhow/thread-events.md`, rather than deriving event names from `system-knowhow/js-sdk/lucidos-media.md` § `lucidos.media`.
  Flag a subscription on the bridge operation as **broken**.

- **Timing updates counted as new waits.** Reconcile `EventWaitTimingUpdated` by wait ID.
  See `system-knowhow/thread-events.md` for the event lifecycle.

- **Ceiling handoffs counted as waiting for input.** Check `SessionHandedOff` subscriptions against `system-knowhow/thread-events.md`.
  Read `starts` rather than inferring it from `cause`: interrupted narratives use `after-this-turn`.
  Flag recipes treating every ceiling handoff as `on-next-input` as **stale**.

- **A ceiling crossing counted as prompt delivery.** Check `ContextCeilingReached` filters against `system-knowhow/thread-events.md`.
  The event records the crossing; safe delivery can happen later.
  Flag recipes treating it as proof that the agent read a handoff prompt as **stale**.

- **Every proposal hold treated as owner action.** Check `ProposalWithheld` filters against `system-knowhow/thread-events/concrete-payload-shapes-selected-3.md` § `ProposalWithheld`.
  Flag recipes asking the owner to act while `agent_returns` is true as **stale**.

- **Applied settings counted as user picks.** `ChatSettingsChanged` records user picks; `ChatTurnSettingsResolved` records turn admission.
  Check subscription intent against `system-knowhow/thread-events.md` before treating automatic admission as a person changing the rung.

- **Follow-ups counted as separate Codex turns.** A capable app-server accepts ordinary follow-ups within its active turn.
  Check receipt and result assumptions against `system-knowhow/coding-agent-events.md`; urgent messages still interrupt.

- **Permission restoration counted as fresh owner consent.** Check permission-event recipes against `system-knowhow/thread-events.md`.
  Flag a recipe treating `PermissionRestored` as a new owner tap or an extension into a later turn as **broken**.

- **Missing sandbox records treated as an opt-out.** Check session filters against `system-knowhow/thread-events.md` and `system-knowhow/agent-sandbox.md`.
  Legacy `SessionStarted` events omit `sandbox`; absence cannot establish that the process ran unsandboxed.
  Check private path edits against the process start time before claiming that a running trigger received them.

- **Memory failures counted per attempt.** `MemoryExtractionFailed` records one exhausted extraction operation, including a deadline expiry.
  An empty facts array is successful. Check filters against `system-knowhow/thread-events.md`.

- **Plan-only changes counted as code landings.** Filters on change events must inspect `content` when they distinguish planning from implementation.
  Use plan kind and approval scope for planning stages, as documented in `system-knowhow/thread-events.md`.
  Filters that count owner decisions exclude events with an `inherited_from_event_id`.
  Exclude `simple-fix` and `bounded-security-fix` approval kinds from those filters too.

- **Billing observations counted as usage.** Check against `system-knowhow/coding-agent-events/billing-evidence.md` § Billing evidence.
  `CodingAgentBillingObserved` is passive evidence; sum usage from `ContextCaptured` only.
  Unknown attribution must not inherit a provider from the model or current settings.
  Restored Codex notifications retain every capture and token count, including repeats.
  Their historical attribution cannot establish billing for new work.

- **Subscriptions to side-question events.** Claude Code, Codex and Lucidos Agent side questions stay hidden from triggers and event waits.
  Flag recipes waiting for those events as **broken**; see `system-knowhow/coding-agent-events/billing-evidence.md` § Side questions are recorded, and hidden from every agent.

- For trigger scripts that write the workspace database, apply `system-knowhow/workspace-audit/scripts`' origin-aware table-policy check.
  Report a refused write once, under its trigger.

- **Successor bookkeeping treated as finished work.** Check recipes using `CodingAgentIdled` or `SessionEnded` as proof that a thread is at rest.
  An eligible automatic successor keeps it running and in In flight; use current summaries and liveness.
  Flag recipes that drop running successor transitions or call an owed successor quiet indefinitely as **stale**.
  A successor a Stop held at handoff is not eligible: it stays quiet until the owner's next message or Continue.

- **A model swap read as a ladder change.** `CodingAgentModelSubstituted` changes no ladder; see `system-knowhow/coding-agent-events/the-full-enumerated-list.md` § Model swaps.
  Flag recipes that treat the swap, or its `model_substituted` re-entry, as the model leaving the ladder as **stale**.

- **Liveness treated as an event subscription.** Thread liveness is derived on read, rather than emitted as a thread event.
  Read it through the SDK or CLI; see `system-knowhow/js-sdk/lucidos-threads-thread-management.md` § `lucidos.threads`.

- **Failure events mistaken for repair starts.** A main-check failure schedules repair work; it does not prove a session started.
  Check repair subscriptions against `system-knowhow/thread-events.md`.
  Flag automation treating a waiting repair as a running thread as **stale**.

- **Child reports missed by wake-only subscriptions.** Check subscriptions that
  need every report for both `ChildThreadCompleted` and `ChildReportFiled`.
  See `system-knowhow/thread-events/concrete-payload-shapes-selected-3.md` § `ChildReportFiled`.

- **Question compose reports treated as fresh user input.** `UserQuestionAnswered` may carry the compose state left by atomic acceptance.
  Consult `system-knowhow/thread-events.md`; the report is replay evidence, not a second answer or authority grant.

- **Saved-draft origin mistaken for owner authority.** Review origin filters against `system-knowhow/thread-events/wire-format-and-metadata.md` § Saved-draft origins.
  `saved_draft` content has Human mode, but its submitting agent and trigger provenance remain distinct from a Device origin.
  Flag recipes treating Human mode alone as an owner-opened turn or an unrestricted spawn grant as **broken**.
  Keep the acceptance event and submitting thread identities when recording provenance; never replace them with a synthetic device.

- **A trigger retrying a provider billing refusal immediately.** Check against `system-knowhow/thread-events/concrete-payload-shapes-selected-2.md` § Provider spend-cap holds.
  Background sends wait during an engine provider hold; coding-agent billing notices do not create those holds.
  Flag recipes that clear the hold repeatedly as **drift**. Direct the owner to Settings → Providers after raising the cap.

Model selectors in trigger-management apps follow `system-knowhow/js-sdk.md`'s
`lucidos.ui.ModelPicker` contract. App selections do not change a trigger's stored configuration automatically.
Compare preference fallbacks using `system-knowhow/js-sdk/lucidos-preferences-user-settings.md` § Common keys, and trigger defaults using `system-knowhow/preferences.md`.

For each finding, capture: **location**, **what's wrong**, **which reference owns the rule** (link, don't quote), **suggested fix**.

**Start from the live projection (`triggers(action="list")` / the trigger registry the scheduler uses), not from `TriggerCreated` events.** Anything off the live list was deleted (`TriggerDeleted`) or no longer fires, so walking events alone invents phantom "broken" triggers. Use `TriggerCreated`/`TriggerUpdated` only to rebuild `run` fields the projection hides (intent text, stale `run.knowhow:[...]`, slug) for live triggers.

Per the engine prompt's taxonomy section and the worked example in `docs/taxonomy.md` (mirrored in best-practices). Trigger threads discover knowhow at fire time via `load_knowhow`, as chat does, with no per-trigger allow-list. For each *live* trigger, reduce its `TriggerCreated` and later `TriggerUpdated` events to the *latest* `run` (most recent payload by sequence), then apply these checks:

- **Knowhow promising unlimited restart replays or no interruption report.** Read each trigger's knowhow against `system-knowhow/triggers/writing-cron-expressions.md` § "A fire due while the engine restarts". Flag instructions that assume scripts always requeue, or that tell a run to duplicate the engine's interruption notification. Severity: **stale**, since the recovery contract bounds retries and owns that report.

- **Knowhow saying the `claude-gateway` proxy serves chat only.** Read each trigger's proxy instructions against `system-knowhow/js-sdk/lucidos-media.md` § `lucidos.proxy`, which owns the supported callers and wire contract. Severity: **stale**.

- **Imperative verbs about *how* in `run.intent`** (hit, parse, scan, fall back, retry, GET, POST, scrape) → procedure leaked into intent.

- **Stale `run.knowhow: [...]` field.** Per `system-knowhow/triggers/running-an-existing-trigger-once-off-sch.md` § "Setup checklist" item 4: the deserializer silently drops legacy `run.knowhow:[...]`. The trigger keeps firing with no knowhow pre-loaded, so behavior depends on discovery finding the same files. Surface each trigger's id and the `knowhow` ids it requested, and recommend the rewrite the source file specifies. Severity: **stale** (silently broken).

- **Subscription on a retired event name.** For each live trigger's `on` list, classify every `event_type` with the `events` tool's `event_types` action. One call returns all three buckets.

  **A rename does not carry subscriptions with it.** The matcher compares the event type as an exact string, so a trigger on a retired name silently stops firing. Only an audit finds these.

  | Bucket the name is in | Verdict |
  |---|---|
  | `engine` | fine, it is live |
  | `retired` | **broken**, it silently stopped firing. Re-point it, see below |
  | `workspace` | fine, it is a domain event this workspace emits itself |
  | none of the three | **smell**, a subscription on a name nothing produces |

  Domain-event names are arbitrary by design, so read the `workspace` bucket first, and never call one a misspelling.

  Payload values are not event types. Keep-alive reasons belong to `CacheKeepAliveStatusChanged`, as `system-knowhow/thread-events.md` documents. Audit the subscription's event name separately from its payload filters.

  **Ask the tool. Never assemble the retired set by hand.** `retired` comes from `ThreadEvent::LEGACY_TYPE_NAME_ALIASES`, which a test holds to the names serde still accepts. A list built by eye from `thread-events.md` misses about half, and adds frontend-only names that were never event renames.

  **`retired` says a name is dead, not what replaced it.** For the successor, search `thread-events.md` for the old name. It sits on its successor's row, as a `Legacy alias:` note or in the row's prose. The whole `ClaudeCode*` family became `CodingAgent*`, as `system-knowhow/coding-agent-events.md` states. If neither resolves it, report the finding without a replacement. A guessed successor arms a second subscription that never fires.

  The engine now refuses a new subscription on a dead name, so this check finds rows armed before that.

- **A subscription on a history-only event.** `OwnerApprovalRequested` and `OwnerApprovalSpent` still read in the `engine` bucket, but nothing emits them since ADR 0688. A trigger on one silently stopped firing. Severity: **broken**. Re-point it to `PermissionRequestPrepared`, `PermissionGranted` or `PermissionSpent`, as their rows in `system-knowhow/thread-events.md` describe.

- **A durable wait or trigger subscription on snapshot progress.**
  `SnapshotProgress` is transient activity, not a persisted completion event.
  Flag a trigger's `on` list or a durable `await_event` waiting for it as
  **broken**. Use the terminal snapshot events documented in
  `system-knowhow/thread-events.md` and read the durable job for the result;
  do not infer success from a progress frame.

- **Retired event names in workspace code.** Same list, different surface, because a stale recipe keeps minting dead subscriptions and the finding above returns after the fix. Grep for retired names in `await_event` calls, `on_event` payloads, and `lucidos triggers` invocations. Search `data/knowhow/**/*.md` (fenced `python` / `bash` / `js` / `ts` blocks), `data/scripts/**`, `data/apps/**`, and `data/triggers/**/scripts/**`. Surface path, line, the retired name and its current name, at severity **stale**.

- **Keep-alive reasons are payload values, not event names.** A `CacheKeepAliveStatusChanged` may carry `provider_paused`, `pings_paused` or `uncacheable_provider` inside `status.reason`. Classify subscriptions by the event name alone. Use `thread-events.md` for each reason's meaning.

- **Missing explicit `slug` field.** The tools, CLI and HTTP API store the slug on `TriggerCreated` (`system-knowhow/triggers/running-an-existing-trigger-once-off-sch.md` § "Setup checklist" item 4). An older event without one re-derives it from the *create-time* name on every read, with no collision check. Recommend an explicit `slug` when the trigger has, or will have, per-trigger knowhow files. Set it with `lucidos triggers update --slug` or the HTTP API, since the LLM tools take none. Severity: **nit** (preventive).

- **Per-trigger knowhow dir orphaned from any live trigger**: for each directory under `data/triggers/<slug>/knowhow/`, confirm `<slug>` matches an active (non-deleted) trigger's slug. The system prompt scopes by exact slug match, so knowhow under an unreferenced slug is invisible. Recommend renaming the directory to a live slug or deleting it. If the trigger runs a **script**, moving its folder also needs `triggers(action="update", run.path=…)`, since the registered path does not follow the folder. Deleting the old folder before that event lands breaks the next fire (see `system-knowhow/triggers/running-an-existing-trigger-once-off-sch.md` § "Renamed trigger → stale `run.path`"). Reference: `system-knowhow/triggers.md`.

- **Notification routing: `tap` opt-ins for CTA-shaped triggers.** Take each trigger whose `run.intent` mentions `send_notification`, plus each `NotificationCreated` event traceable to a trigger. Look the body's shape up in the table under `system-knowhow/triggers/actor.md` § "Notification routing". Report each trigger whose `app_id`, `tap` or `event_id` disagrees with its row, quoting the row as the fix. Severity: **drift**, since the default works and the opt-in only tightens UX. Skip a trigger that already sets `tap` to a non-default value.

- **An MCP server a trigger uses but does not list.** A trigger's fires may call only the MCP servers in its `mcp_servers` (`system-knowhow/triggers/grouping-triggers.md` § "MCP servers the fires use: `mcp_servers`"). A trigger that used one before that rule, or whose fire enabled one mid-turn, now has those calls refused. Find each pair over the last 30 days:

  ```bash
  psql "$LUCIDOS_DATABASE_URL" -At -F ' ' -c "SELECT ts.trigger_id, coalesce(e.payload->>'server_id', split_part(e.payload->>'name', '__', 2)) AS server_id, count(DISTINCT e.thread_id) FROM events e JOIN thread_summaries ts ON ts.thread_id = e.thread_id WHERE ts.trigger_id IS NOT NULL AND e.created >= now() - interval '30 days' AND ((e.event_type = 'ToolCalled' AND e.payload->>'name' LIKE 'mcp\_\_%') OR (e.event_type = 'ThreadMcpServerEnabled' AND e.payload->>'by' = 'agent')) GROUP BY 1, 2"
  ```

  Compare each row with that live trigger's `mcp_servers` from `lucidos triggers list`. Report every server it used but does not list, with the fires that used it. Severity: **smell**, since each fire now fails that call loud, with a notification. This is the ADR 0214 migration check for ADR 0430.

- **A trigger that starts coding agents without the coding-agent grant.** A trigger's runs may start a coding-agent thread only with that grant (`system-knowhow/triggers/grouping-triggers.md` § "Coding-agent grant"). Find each trigger whose runs have started one. The query follows each thread back to its cause, as the engine's check does, so top-level and CLI spawns count. It follows a saved draft to the thread that submitted it. It also follows the parent link, which older threads carry alone:

