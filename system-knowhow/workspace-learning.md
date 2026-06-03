---
name: Workspace Learning
description: Recipe for learning from a workspace's recent events — proposing improvements to apps, triggers, knowhow, and scripts based on observed friction. Use when the user asks what to learn from the runtime, where things aren't working well, or what patterns the events show.
---

# Workspace Learning

A read-only sweep of a Lucidos workspace's recent **events** that surfaces recurring friction and proposes targeted edits to the workspace's apps, triggers, knowhow, and scripts. Output: one Markdown report under `data/artifacts/learning/` plus a `WorkspaceLearningCompleted` event.

This is the sibling of `workspace-audit`. Audit checks the workspace against today's rules ("you drifted"). Learning checks today's runtime against itself ("the rules — or the way you've encoded intent — might be wrong"). The proposed fix often edits the very files audit treats as ground truth.

## When to run this

User says "what can we learn", "review the runtime", "what's not working well", "any patterns in failures", "see what to improve", "look at the last week and tell me what to fix". Or fired on a cadence by a scheduled trigger the user has set up.

**Read-only.** Never edit, delete, retry, or re-run anything during the analysis. The report proposes fixes; the user decides what to apply, and routing follows the scope tag (see "Routing").

## Sources of truth — load these first

This knowhow does **not** restate the rules; it reads runtime data and points at the rules to suggest where a fix would live. When a finding implies an edit, the suggested-fix line should name the file that owns the convention so the user (or a follow-up session) knows where to land it.

| Reference | Owns |
|---|---|
| `system-knowhow/best-practices.md` | Workspace file conventions and the intent-vs-knowhow split |
| `system-knowhow/js-sdk.md` | App SDK surface — what an app *can* call vs. what it tried |
| `system-knowhow/lucidos-cli.md` | CLI for `data.*` writes and `events.*` emits used by scripts |
| The active engine system prompt | Trigger taxonomy, how knowhow is loaded into context |

When you suggest a fix, point at the file — don't quote it.

## What to walk

Window: last **N days** (default 7; user may say "last 30 days" or "since last release"). From chat use `query_events` / `count_events`; from a script use `lucidos events query` / `lucidos events count`.

**Count first, then drill** — this is mandatory, not advisory. A busy week can easily produce 2 MB+ of `ToolResult` payloads, and chaining many `query_events` calls in a single turn will blow the LLM's prompt budget on the next turn (the recipe and trigger that owns this file did exactly that on 2026-05-25 — 8 calls × 256 KB tool results → 1.54 M tokens to a 1 M-cap API → `prompt is too long` failure). The workflow is:

1. **Size the window.** Call `count_events` once with `since` set to the window start and **no** `event_type` filter. You'll get a per-type breakdown (`{by_type: [{event_type, count, byte_total}, ...]}`) sorted by count desc. `byte_total` is the raw payload byte sum and is the right proxy for "how expensive will it be to pull this into context".
2. **Decide what to drill into.** Skip any friction-signal type with `count < 3` (the threshold rule below). For types above threshold, sort by `byte_total` and plan accordingly — high-byte types (`ToolResult`, `CodingAgentToolCalled`, `TextStreamed`) need especially tight `limit` values.
3. **Drill narrow.** For each type you keep, call `query_events` with the `event_type` filter and `limit: 50` (the engine default — sampling, not enumeration). The engine hard-caps `limit` at 200 and `byte_limit` at 512 KB; the LLM-tool description states these. Never call `query_events` without an `event_type` filter on a window above ~24 hours.
4. **Watch for `truncated:true`.** `query_events` returns `{events, total_matching, returned, byte_size, truncated, hint?}` and honours a 128 KB default `byte_limit`. When `truncated:true`, follow the hint: narrow by `aggregate_id` or shorten the time window. Do not retry with a larger `byte_limit` unless you've already narrowed.
5. **Soft cap of 3 `query_events` calls per assistant turn.** The engine bounds each call but does NOT track per-turn totals — 5+ calls in one turn (one assistant message with many tool_use blocks) can still accumulate enough tool-result bytes to blow the next turn's prompt budget. If you have more than 3 friction-signal types above threshold, split the drill across multiple turns; the trigger thread persists and can resume.

**Pagination is an anti-pattern here.** The whole point of count-first-then-sample is that you never need to enumerate every event of every type — clustering is the goal, exhaustion is not.

Friction signals to pull:

| Signal | Event types |
|---|---|
| Tool failures | `ToolResult` payloads with errors; repeated `ToolCalled` to the same target without success. HTTP-tool failures surface here too — there is no dedicated HTTP event. |
| Circuit-breaker trips | LLM force-broken on the same target ≥3 times in one thread (see `.claude/rules/frontend.md` § Circuit Breakers) |
| Failed responses | `ResponseFailed` payloads — model errors, timeouts, parsing failures. Group by error reason. |
| Aborts / cancels | `ResponseAborted` (system) and `ResponseCanceled` (user) — both worth grouping. |
| Trigger failures | `TriggerCompleted` payloads with errors, or triggers that ran but produced no useful output |
| Dead triggers | `TriggerCreated` with zero matching `TriggerCompleted` since creation |
| App errors | App emits its own error events, or `ToolResult` errors in app-spawned threads |
| User corrections in chats | `MessageReceived` immediately following a `ResponseGenerated` / `ResponseAborted` / `ToolResult` whose text reads like a correction ("no", "don't", "stop", "actually", "that's wrong", "instead", "you misunderstood") |
| CC sessions ending without a useful change | `CodingAgentIdled` followed by `ChangeDiscarded`, or no `ChangeProposed` at all when one was clearly expected |
| Engine crashes / supervisor respawns | `EngineSupervisorRespawned` — bash supervisor logged the previous engine pid died with a non-graceful exit (SIGKILL, panic, OOM, process-group kill). Payload carries `exit_code` (137 = SIGKILL, 143 = SIGTERM via 128+N, etc.) and `died_at`. Always `[engine]` scope. A single occurrence in the window is reportable even below the ≥3 threshold (catastrophic-single rule); cluster + report whenever the engine had to be respawned at all. |

Trigger intent text lives in `TriggerCreated` payloads (`run.intent`) — pull it when assessing trigger findings.

## What to check

For each finding capture: **pattern**, **scope** (`[workspace]` or `[engine]`), **count + window**, **examples** (event ids or aggregate ids), **likely cause** (one sentence), **where the fix lives** (file path, not quote), **suggested fix** (terse).

### Scope tag

Use the *where the fix lives* line as the test. A finding is `[workspace]` when the fix lands in this workspace's content (a knowhow, app, trigger, script, intent, or the system prompt's workspace-specific section). A finding is `[engine]` when the fix lands in engine code, engine config, the upstream model surface, or the SDK/CLI itself — i.e. nothing in `data/` or `system-knowhow/` will resolve it. When a pattern category below names an explicit scope, follow it. When unsure and no category rule applies, default to `[workspace]` and add a separate `**Scope note:** unclear — could be either` line; the user can re-tag.

The audience differs: `[workspace]` findings get actioned in the workspace itself; `[engine]` findings are diagnostic — they get filed against the Lucidos source repo, not fixed in the workspace. Don't drop engine findings just because the workspace can't fix them. See "Routing" below for who actually does the work in each case.

### Routing — who actions each scope

Scope tag *is* the routing decision; never re-route by hand.

- **`[workspace]` → Lucidos handles it.** The Lucidos LLM has the tools needed to edit knowhow, trigger configs, app code, intents, and repo registration. Action through a regular Lucidos chat thread, not Claude Code.
- **`[engine]` → Claude Code handles it.** Findings land in the Lucidos source repo (Rust crates, engine config, SDK/CLI surface, `system-knowhow/` itself). Action via `run_claude` against the Lucidos repo.

A `[workspace]` finding never goes to CC; an `[engine]` finding never goes to a Lucidos chat.

### Noise filter

Apply a **threshold of ≥3 occurrences** before reporting a pattern (or an obviously catastrophic single event — engine crash, data loss). One-off failures get a pass. The point is to find recurring shapes, not to log every error. If a category has only singletons, list it as "no pattern" rather than dumping the events.

### Already-fixed check — REQUIRED

Before including a finding in the report, verify it has not already been addressed since the friction occurred. Without this check the recipe surfaces stale findings (e.g. an MCP timeout that was fixed mid-window reported as if it's open). This is a hard requirement, not a suggestion.

**Cutoff is the timestamp of the first occurrence of the pattern in the window** — not the start of the window. Occurrences before a known fix are noise; occurrences after the fix are the real signal.

- **`[engine]` findings:** check git log in the Lucidos repo for commits since the first-occurrence timestamp that touch the relevant area (file path, error string, subsystem). `git log --since=<first-occurrence-iso8601> -- <path-or-area>`. If a plausible fix has landed, drop the finding or annotate with `**Already fixed:** <commit sha> — <subject>` and report only the post-fix occurrence count.
- **`[workspace]` findings:** check whether the relevant workspace file (knowhow, trigger config, app code, intent) has been edited since the first occurrence. Use `git log --since=<first-occurrence-iso8601> -- <path>` in the workspace repo, or the file's mtime if not git-tracked. If the edit plausibly addresses the pattern, drop or annotate as above.
- **If post-fix occurrences fall below the ≥3 threshold, drop the finding entirely.** A pattern that was real, then fixed, then quiet is not a finding.

### Pattern categories

#### 1. Recurring tool failures

Same tool + same error signature across ≥3 calls (any thread). Common causes: knowhow telling the LLM to call the tool with the wrong shape; missing precondition the knowhow forgot; deprecated tool surface. Fix usually lives in the knowhow that prompts the call.

#### 2. Circuit-breaker trips

LLM looping on the same target. Almost always a sign that a knowhow doesn't tell the LLM how to *stop* — no fallback, no "if X, do Y instead" branch. Fix lives in the knowhow that drives the loop.

#### 3. Trigger failures clustered on one trigger

`TriggerCompleted` errors clustered on one `aggregate_id`. Read its `TriggerCreated`. If `run.intent` carries imperative how-to, the LLM is improvising the procedure each run and failing differently each time — the fix is to lift the procedure into a knowhow file the trigger thread will discover via `load_knowhow` (per-trigger knowhow lives at `data/triggers/<slug>/knowhow/`). If `run.intent` is fine, the fix is in whichever knowhow the trigger thread is (or should be) loading.

#### 4. Dead triggers

`TriggerCreated` with no successful run since creation (≥7 days old). Either the schedule never fires, the precondition is never true, or the user forgot about it. Suggest archive or repair — don't auto-delete.

#### 5. Repeated user corrections in chats — `[workspace]`

When the same correction shape recurs across threads ("the report is too long", "stop using bullet lists", "you keep summarizing what I just said"), the issue is likely in a globally-loaded knowhow or the workspace-specific section of the system prompt, not in any one chat. Cluster by correction theme, point at the prompt/knowhow that would carry the fix. (If the correction is about general engine behavior rather than this workspace's content — e.g. model verbosity defaults — tag `[engine]`.)

#### 6. Failed responses, aborts, and cancels — splits

- `ResponseFailed` / `ResponseAborted` → `[engine]`. Engine-side issues (model errors, timeouts, parse failures); group by error reason where the payload carries one. Fix lives in engine config or upstream retry logic, not in workspace content.
- `ResponseCanceled` → `[workspace]`. User is stopping the LLM mid-stride; read the surrounding context, infer what made them pull the plug, propose a knowhow change that gets the LLM there faster.

#### 7. CC sessions that produce nothing

`CodingAgentIdled` with no `ChangeProposed` (or a `ChangeDiscarded` immediately after Apply isn't reached) on a recurring task suggests the knowhow that drives that CC flow is unclear about what success looks like. Fix lives in the knowhow.

#### 8. App-level friction — splits

App misusing the SDK/CLI surface (calling something the docs already cover correctly) → `[workspace]`. Fix lives in the app's own code or knowhow. SDK/CLI *shape drift* (the actual surface diverged from `system-knowhow/js-sdk.md` or `system-knowhow/lucidos-cli.md`, so apps are right and the docs/engine are wrong) → `[engine]`. The tell: does updating the app fix it, or does the app need a surface that doesn't exist yet?

## Output

Write to `data/artifacts/learning/YYYY-MM-DD-HHMM/report.md` (user's local time; UTC if timezone unknown). Use `lucidos data write` so it lands in the workspace, not the worktree.

### Report structure

```markdown
# Workspace Learning — YYYY-MM-DD HH:MM

## Summary
- Window: <N> days, <total events scanned>
- <K> patterns surfaced across <C> categories (<W> workspace / <E> engine)
- Categories with no pattern (below threshold or clean): <list>

## <Category>
### <pattern> `[workspace|engine]` — <count> in <window>
**Examples:** <event ids or aggregate ids, up to 3>
**Likely cause:** <one sentence>
**Where the fix lives:** `<path>`
**Suggested fix:** <terse>
**Already fixed:** <commit sha> — <subject>   ← only if a partial fix has landed; report only post-fix occurrences
```

Order categories by count, descending. Within a category, order patterns by count. Within a count-tie, `[workspace]` before `[engine]` — actionable findings first.

### Event

```bash
lucidos events emit WorkspaceLearningCompleted \
  --summary "Workspace learning: <K> patterns across <C> categories (window <N>d)" \
  --payload '{"artifact": "artifacts/learning/<dir>/report.md", "patterns": <K>, "categories": <C>, "workspace_findings": <W>, "engine_findings": <E>, "window_days": <N>, "events_scanned": <T>}'
```

## Out of scope

- **No edits or deletes.** Suggested fixes only. Acting on the report is a separate step, routed by scope tag (see "Routing").
- **No retries or re-runs.** Don't replay failed tool calls or re-fire triggers to "see if it still fails".
- **No compliance checks against current conventions** — that's `workspace-audit`. If a learning finding implies the workspace also drifted from a rule, note it in passing and suggest running audit; don't expand into rule-checking.
- **No per-event enumeration.** Cluster, count, give examples; never paste the full event payload list. When summarizing user-correction patterns, summarize the *shape* of the correction, not the user's text verbatim.
- **No engine codebase analysis** — this learns from the workspace's runtime, not from `crates/`.

## Idempotency

Each run gets its own timestamped directory. Don't overwrite previous reports — diffing them shows whether the same patterns keep coming back (a sign the proposed fix wasn't applied, or didn't work). On same-minute collision, append a counter.

## Maintenance

When event types are added, renamed, or retired (especially the friction signals in "What to walk"), this recipe's checks may go stale. See the `Maintaining workspace-learning` section in the repo's `CLAUDE.md` for the rule that governs when this file must be updated alongside event schema changes.
