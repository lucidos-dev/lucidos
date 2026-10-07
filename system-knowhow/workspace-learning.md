---
name: Workspace Learning
description: Recipe for learning from a workspace's recent events, proposing improvements to apps, triggers, knowhow and scripts based on observed friction. Use when the user asks what to learn from the runtime, where things aren't working, or what patterns the events show.
---

# Workspace Learning

A read-only sweep of a workspace's recent **events**. It finds recurring friction and proposes targeted edits to apps, triggers, knowhow and scripts. Output: one Markdown report under `data/artifacts/learning/` plus a `WorkspaceLearningCompleted` event.

This is the sibling of `workspace-audit`. Audit checks the workspace against today's rules ("you drifted"). Learning checks today's runtime against itself ("the rules, or the way you encoded intent, might be wrong"). The fix often edits the very files audit treats as ground truth.

## When to run this

User says "what can we learn", "review the runtime" or "what's not working well". Or "any patterns in failures", "see what to improve", "look at the last week and tell me what to fix". Or a scheduled trigger the user set up fires it on a cadence.

**Read-only.** Never edit, delete, retry or re-run anything during the analysis. The report proposes fixes. The user decides what to apply, and the scope tag sets the routing (see "Routing").

## Sources of truth: load these first

This knowhow does **not** restate the rules. It reads runtime data and points at the file that owns each convention. Name that file on the suggested-fix line, so the user or a follow-up session knows where to land the fix.

| Reference | Owns |
|---|---|
| `system-knowhow/best-practices.md` | Workspace file conventions and the intent-vs-knowhow split |
| `system-knowhow/js-sdk.md` | App SDK surface: what an app *can* call vs. what it tried |
| `system-knowhow/lucidos-cli.md` | CLI for `data.*` writes and `events.*` emits used by scripts |
| The active engine system prompt | Trigger taxonomy, how knowhow is loaded into context |

Point at the file. Don't quote it.

## What to walk

Window: a recurring run (a trigger fire) covers everything since its previous run. Read that time from this trigger's `Last run:` line in `list_triggers`, which is UTC. The engine stamps it when a fire finishes, pass or fail, so the current fire never shows there. A failed fire writes no report. A successful fire emits `WorkspaceLearningCompleted` minutes before `Last run:`. If the newest one is more than an hour older, that fire failed, so start from the event.

Cap the window at 30 days. After a longer gap, such as a paused trigger, start 30 days back and say so in the report Summary. Friction older than that is almost always fixed or stale.

Fall back to the last **N days** (default 7) when `Last run:` reads `never`, or for a one-off chat run. A window the user names ("last 30 days", "since last release") wins. From chat use `query_events` / `count_events`; from a script use `lucidos events query` / `lucidos events count`.

**Count first, then drill. This is mandatory.** A busy week can produce 2 MB+ of `ToolResult` payloads. Chaining many `query_events` calls in one turn blows the next turn's prompt budget. This recipe once sent 1.54 M tokens to a 1 M-cap API that way and failed with `prompt is too long`.

1. **Size the window.** Call `count_events` once with `since` at the window start and **no** `event_type` filter. It returns `{by_type: [{event_type, count, byte_total}, ...]}`, sorted by count desc. `byte_total` is the raw payload byte sum, the right proxy for context cost.
2. **Decide what to drill into.** Skip any friction-signal type with `count < 3` (see "Noise filter"). Sort the rest by `byte_total`. High-byte types (`ToolResult`, `CodingAgentToolCalled`, `TextStreamed`) need especially tight `limit` values.
3. **Drill narrow.** For each type you keep, call `query_events` with the `event_type` filter and `limit: 50` (the engine default: sample, don't enumerate). The engine hard-caps `limit` at 200 and `byte_limit` at 512 KB. Never call `query_events` without an `event_type` filter on a window above ~24 hours.
4. **Watch for `truncated:true`.** `query_events` returns `{events, total_matching, returned, byte_size, truncated, hint?}` with a 128 KB default `byte_limit`. On `truncated:true`, follow the hint: narrow by `aggregate_id` or shorten the window. Raise `byte_limit` only after you narrowed.
5. **Soft cap of 3 `query_events` calls per assistant turn.** The engine bounds each call but does NOT track per-turn totals. Five or more calls in one assistant message can still blow the next turn's budget. With more than 3 types above threshold, split the drill across turns. The trigger thread persists and can resume.

**Step 1 sizes the drill and never produces a finding on its own.** A high count means a type is expensive to pull, nothing more. Event volume is not friction, and neither is a trigger on a tight cadence. The event log is the workspace's memory, so a quieter log is not a better one.

Trigger lifecycle events are the clearest case. Every fire emits `ThreadQueued`, `ThreadQueueAdmitted`, `TriggerExecuted`, `TriggerCompleted` and `ThreadQueueCompleted`: the record of what ran. A trigger woken by another trigger's completion is not an amplifier either. A trigger never wakes on its own fire, so a broad `TriggerCompleted` subscription is the intended shape. Report a trigger only for a signal in the friction table below.

**Don't paginate.** Count-then-sample means you never enumerate every event. The goal is clusters, not exhaustion.

**To re-read ONE event, pass its `event_id`** instead of searching again. It takes the `evt-<32 hex>` address a tool result states, or a bare uuid. It is a primary-key read: exact and cheap. A tool call's address returns the pair, call then result, because the result is the half that left context. It still honours `byte_limit`, which matters for a multi-megabyte result. An address that matches nothing errors, so a stale pointer never reads as an empty window.

Friction signals to pull:

| Signal | Event types |
|---|---|
| Tool failures | `ToolResult` payloads with errors; repeated `ToolCalled` to the same target without success. HTTP-tool failures land here too: there is no dedicated HTTP event. |
| Circuit-breaker trips | LLM warned (3 consecutive failures) or force-broken (5) on the same failing target in one thread. The breaker gates on repeated *failure*, not repeated call (see `.claude/rules/frontend.md` § Circuit Breakers) |
| Failed responses | `ResponseFailed` payloads: model errors, timeouts, parse failures. Group by error reason. |
| Aborts / cancels | `ResponseAborted` (system) and `ResponseCanceled` (user). Group both. |
| Trigger failures | `TriggerCompleted` payloads whose `result_summary` carries an error. The engine never leaves `result_summary` empty. A run with no output falls back to `"<name> completed (no output)"` or `"<name> completed (exit <code>, no output)"`. Those lines are an **expected** idle-detector outcome (broad subscription + cheap internal gate), **not** friction. Never flag them as "produced no useful output". |
| Dead triggers | `TriggerCreated` with zero matching `TriggerCompleted` since creation |
| App errors | App emits its own error events, or `ToolResult` errors in app-spawned threads |
| User corrections in chats | `MessageReceived` immediately following a `ResponseGenerated` / `ResponseAborted` / `ToolResult` whose text reads like a correction ("no", "don't", "stop", "actually", "that's wrong", "instead", "you misunderstood") |
| Coding-agent sessions ending without a useful change | `CodingAgentIdled` followed by `ChangeDiscarded`, or no `ChangeProposed` at all when one was clearly expected |
| Engine crashes / supervisor respawns | `EngineSupervisorRespawned`: the bash supervisor saw the previous engine pid die with a non-graceful exit (SIGKILL, panic, OOM, process-group kill). Payload carries `exit_code` (137 = SIGKILL, 143 = SIGTERM via 128+N, etc.) and `died_at`. Always `[engine]` scope. One occurrence is reportable below the ≥3 threshold (catastrophic-single rule). |

Trigger intent text lives in `TriggerCreated` payloads (`run.intent`). Pull it when you assess trigger findings.

## What to check

For each finding capture: **pattern**, **scope** (`[workspace]` or `[engine]`), **count + window**, **examples** (event ids or aggregate ids), **likely cause** (one sentence), **where the fix lives** (file path, not quote), **suggested fix** (terse).

### Scope tag

The *where the fix lives* line is the test. A finding is `[workspace]` when the fix lands in this workspace's content: a knowhow, app, trigger, script, intent, or the system prompt's workspace-specific section. It is `[engine]` when the fix lands in engine code, engine config, the upstream model surface, or the SDK/CLI itself. Nothing in `data/` or `system-knowhow/` resolves an `[engine]` finding. When a pattern category below names a scope, follow it. Otherwise, when unsure, default to `[workspace]` and add a `**Scope note:** unclear, could be either` line for the user to re-tag.

Keep `[engine]` findings even though the workspace can't fix them. They are diagnostic, and they get filed against the Lucidos source repo.

### Routing: who actions each scope

The scope tag *is* the routing decision. Never re-route by hand.

- **`[workspace]` → Lucidos handles it.** The Lucidos LLM has the tools to edit knowhow, trigger configs, app code, intents and repo registration. Action it in a regular Lucidos chat thread, never a coding-agent thread.
- **`[engine]` → a coding agent handles it.** The fix lands in the Lucidos source repo (Rust crates, engine config, SDK/CLI surface, `system-knowhow/` itself). Action it via `run_coding_agent` against the Lucidos repo, never a Lucidos chat, and only on an install launched from a source checkout. A packaged install has no platform source and refuses that spawn. There, only *report* the finding, and tell the user it needs a workspace running from a source checkout. Don't downgrade it to `[workspace]` to make it actionable.

### Noise filter

Report a pattern only at **≥3 occurrences**, or for an obviously catastrophic single event (engine crash, data loss). The goal is recurring shapes, not a log of every error. If a category has only singletons, list it as "no pattern" instead of dumping the events.

### Already-fixed check: REQUIRED

Before you include a finding, verify nobody fixed it since the friction occurred. Without this check the report surfaces stale findings, such as an MCP timeout fixed mid-window.

**The cutoff is the first occurrence of the pattern in the window**, not the window start. Occurrences before a known fix are noise. Occurrences after it are the real signal.

- **`[engine]` findings:** look in the Lucidos repo for commits since the first occurrence that touch the area (file path, error string, subsystem). `git log --since=<first-occurrence-iso8601> -- <path-or-area>`. If a plausible fix landed, drop the finding or annotate it with `**Already fixed:** <commit sha> (<subject>)`. Then report only the post-fix occurrence count.
- **`[workspace]` findings:** check whether someone edited the relevant workspace file (knowhow, trigger config, app code, intent) since the first occurrence. Use `git log --since=<first-occurrence-iso8601> -- <path>` in the workspace repo, or the file's mtime if git doesn't track it. If the edit plausibly addresses the pattern, drop or annotate as above.
- **If post-fix occurrences fall below 3, drop the finding entirely.** A pattern that was real, then fixed, then quiet is not a finding.

### Pattern categories

#### 1. Recurring tool failures

Same tool + same error signature across ≥3 calls (any thread). Common causes: knowhow tells the LLM to call the tool with the wrong shape, knowhow forgot a precondition, or the tool surface is deprecated. The fix usually lives in the knowhow that prompts the call.

#### 2. Circuit-breaker trips

The LLM loops on the same target. Almost always a knowhow doesn't tell it how to *stop*: no fallback, no "if X, do Y instead" branch. The fix lives in the knowhow that drives the loop.

#### 3. Trigger failures clustered on one trigger

`TriggerCompleted` errors clustered on one `aggregate_id`. Read its `TriggerCreated`. If `run.intent` carries imperative how-to, the LLM improvises the procedure each run and fails differently each time. Lift the procedure into a knowhow file the trigger thread finds via `load_knowhow` (per-trigger knowhow lives at `data/triggers/<slug>/knowhow/`). If `run.intent` is fine, the fix is in the knowhow the trigger thread loads, or should load.

#### 4. Dead triggers

`TriggerCreated` with no successful run since creation (≥7 days old). The schedule never fires, the precondition is never true, or the user forgot it. Suggest archive or repair. Never auto-delete.

#### 5. Repeated user corrections in chats: `[workspace]`

The same correction shape recurs across threads ("the report is too long", "stop using bullet lists", "you keep summarizing what I just said"). The cause is likely a globally-loaded knowhow or the system prompt's workspace-specific section, not one chat. Cluster by correction theme and point at the prompt or knowhow that would carry the fix. Tag `[engine]` when the correction is about general engine behavior, such as model verbosity defaults.

#### 6. Failed responses, aborts, and cancels: splits

- `ResponseFailed` / `ResponseAborted` → `[engine]`. Engine-side issues (model errors, timeouts, parse failures). Group by error reason where the payload carries one. The fix lives in engine config or upstream retry logic, not workspace content.
- `ResponseCanceled` → `[workspace]`. The user stopped the LLM mid-stride. Read the surrounding context, infer why, and propose a knowhow change that gets the LLM there faster.

#### 7. Coding-agent sessions that produce nothing

`CodingAgentIdled` with no `ChangeProposed` on a recurring task, or a `ChangeDiscarded` before Apply. Either suggests the knowhow behind that coding-agent flow is unclear about what success looks like. The fix lives in that knowhow.

#### 8. App-level friction: splits

- App misuses the SDK/CLI surface (calls something the docs already cover correctly) → `[workspace]`. The fix lives in the app's own code or knowhow.
- SDK/CLI *shape drift* → `[engine]`. The real surface diverged from `system-knowhow/js-sdk.md` or `system-knowhow/lucidos-cli.md`, so the apps are right and the docs or engine are wrong.

The tell: does updating the app fix it, or does the app need a surface that doesn't exist yet?

## Output

Write to `data/artifacts/learning/YYYY-MM-DD-HHMM/report.md` (user's local time; UTC if timezone unknown). Use `lucidos data write` so it lands in the workspace, not the worktree.

### Report structure

```markdown
# Workspace Learning: YYYY-MM-DD HH:MM

## Summary
- Window: <N> days, <total events scanned>
- <K> patterns surfaced across <C> categories (<W> workspace / <E> engine)
- Categories with no pattern (below threshold or clean): <list>

## <Category>
### <pattern> `[workspace|engine]`: <count> in <window>
**Examples:** <event ids or aggregate ids, up to 3>
**Likely cause:** <one sentence>
**Where the fix lives:** `<path>`
**Suggested fix:** <terse>
**Already fixed:** <commit sha> (<subject>)   ← only if a partial fix has landed; report only post-fix occurrences
```

Order categories by count, descending, and patterns within a category by count. On a tie, `[workspace]` comes before `[engine]`, so actionable findings lead.

### Event

```bash
lucidos events emit WorkspaceLearningCompleted \
  --summary "Workspace learning: <K> patterns across <C> categories (window <N>d)" \
  --payload '{"artifact": "artifacts/learning/<dir>/report.md", "patterns": <K>, "categories": <C>, "workspace_findings": <W>, "engine_findings": <E>, "window_days": <N>, "events_scanned": <T>}'
```

## Out of scope

- **No edits or deletes.** Suggested fixes only. Acting on the report is a separate step, routed by scope tag (see "Routing").
- **No retries or re-runs.** Don't replay failed tool calls or re-fire triggers to "see if it still fails".
- **No compliance checks against current conventions.** That is `workspace-audit`. If a finding implies the workspace also drifted from a rule, note it in passing and suggest running audit.
- **No per-event enumeration.** Cluster, count, give examples. Never paste the full event payload list. For user corrections, summarize the *shape* of the correction, not the user's text.
- **No engine codebase analysis.** This learns from the workspace's runtime, not from `crates/`.

## Idempotency

Each run gets its own timestamped directory. Never overwrite a previous report: diffing them shows whether a pattern keeps coming back, so the fix wasn't applied or didn't work. On a same-minute collision, append a counter.

## Maintenance

Adding, renaming or retiring an event type can stale this recipe, especially the friction signals in "What to walk". The `Maintaining workspace-learning` section of the repo's `.claude/rules/system-knowhow.md` says when this file must change with the event schema.
