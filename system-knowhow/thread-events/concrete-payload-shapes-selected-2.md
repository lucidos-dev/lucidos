---
name: ThreadEvent Reference, Concrete payload shapes (selected) (continued)
description: Part of ThreadEvent Reference. Its index lists the sections it covers.
---

One part of `system-knowhow/thread-events`, which lists every part.

## Concrete payload shapes (selected) (continued)

### `BackgroundBashStarted` / `BackgroundBashCompleted`

```json
{
  "type": "BackgroundBashStarted",
  "data": {
    "task_id": "bash-7f2c…",
    "command": "cargo test -p lucidos-engine --lib",
    "timeout_secs": 600,
    "started_at": "2026-05-13T18:23:01Z"
  }
}
```

```json
{
  "type": "BackgroundBashCompleted",
  "data": {
    "task_id": "bash-7f2c…",
    "command": "cargo test -p lucidos-engine --lib",
    "exit_code": 0,
    "stdout": "running 1842 tests …",
    "stderr": "",
    "started_at": "2026-05-13T18:23:01Z",
    "finished_at": "2026-05-13T18:25:47Z",
    "timed_out": false,
    "killed": false
  }
}
```

`stdout` / `stderr` are capped at 100 KB each and keep the **tail** when they
overflow. The end of a build log is where the failure is. A leading
marker opens `[truncated`, then a dash, then
`N earlier bytes dropped, showing the most recent M of T total]` verbatim. `bash_output` renders the same tail when it
falls back to this row. So the live drain and the archive never disagree. `N`
also counts what the engine's ~2 MB per-stream ring buffer discarded, so it is
the real gap.

An *event wait* that delivers this event cuts each stream to its last 4000
bytes in the prompt that re-opens the thread. The full row stays in the event
log, and `bash_output` / `lucidos background-task output` read it.

**Reading the status.** `exit_code` and `signal` are mutually exclusive, and neither ever stands in for a status the engine did not obtain:

| `exit_code` | `signal` | Meaning |
|---|---|---|
| `0` | absent | The command really exited 0. A reader can trust this. |
| non-zero | absent | Normal exit with that status. |
| `null` | set | The child was terminated by that Unix signal: `15` SIGTERM (the watchdog timeout and `bash_kill` send it first), `9` SIGKILL (their follow-up after a 3 s grace, and the teardown), `11` SIGSEGV, `13` SIGPIPE (a pipeline producer whose consumer closed the pipe). |
| `null` | absent | The engine could not determine the status. Treat as failure, never as success. Also the shape of rows written before `signal` existed. |

`signal` is omitted from the payload when there is none. So is `abandoned` when false.

**`abandoned: true` means the engine went away, not that the task failed.** A background task is a child of the engine process. A restart, a crash or an OOM ends it, and nobody is left to reap a status.

The engine records that from two places. Graceful teardown kills each running task, waits for the reap, and writes the completion with all its output. The next boot sweeps up what teardown could not reach: every SIGKILL, OOM, panic and power cut. Both rows carry `abandoned: true`, no `exit_code` and no `signal`.

That keeps the promise in `system-knowhow/running-python.md` that ending a turn with background work running is a valid wait. The subscription resolves at the next boot, not at its own deadline.

**The `stderr` line says which path wrote the row.** Teardown killed the task and kept its output. The boot sweep kept neither, since no destructor ran after the crash.

**Neither promises the work stopped.** A crash kills nothing. The teardown's SIGKILL reaches the task's whole process group, but misses a process that detached into its own session. So either path can leave a child running under init. Check before re-running the same work.

`abandoned` is distinct from `killed`, which means `bash_kill` was called, and outranks it when the engine's own shutdown sent the signal. Reading one as the other says someone called the work off. On the boot path, `finished_at` is when the loss was recorded, so it spans the downtime and is not the task's runtime.

**A failing pipeline stage is never masked by a later succeeding one.** The engine runs commands under `bash -o pipefail`, so `cargo clippy … 2>&1 | tee build.log` reports clippy's `101`, not `tee`'s `0`. Without `pipefail`, failing builds silently reported `exit_code: 0`. `pipefail` reports the *rightmost failing* stage (`sh -c 'exit 42' | sh -c 'exit 7'` → `7`). So it shows that a pipeline failed, not always which stage. On a host with no `bash` the engine falls back to `/bin/sh`, logs `[Shell] no bash found …`, and the guarantee does not hold.

Both events also fire for `run_python_background`. The `command` then carries the venv-rooted python invocation, e.g. `'/<ws>/.lucidos/runtime/python/venv/bin/python' '/<ws>/.lucidos/exhaust/<run_id>/script.py'`. The script stays under `.lucidos/exhaust/`, so the audit trail records which one ran. One registry, one event pair, one watcher: consumers never branch on the spawning tool.

### `ContextCaptured`

```json
{
  "type": "ContextCaptured",
  "data": {
    "producer": "main_llm",
    "model": "claude-opus-4-7",
    "context_window": 200000,
    "sections": [
      { "name": "System Instructions", "budget_delta_chars": 49380, "content_chars": 49380, "role": "system" },
      { "name": "Memory", "budget_delta_chars": 1690, "content_chars": 1690, "role": "user", "group": "Memory & history" }
    ],
    "tools": ["bash", "read", "edit"],
    "estimated_total_tokens": 20428,
    "usage": { "input_tokens": 19878, "output_tokens": 0, "cache_read_tokens": 0, "cache_creation_tokens": 0 },
    "trimmed": false
  }
}
```

`producer` is `ContextProducer`, serialized snake_case: `main_llm`, `claude_code`, `codex`, `auxiliary`. Match those exact values in a `condition:` filter; the PascalCase Rust names never match. `usage` is `None` pre-call and on a call whose provider reported no prompt tokens. When present it carries the real provider-reported counts. **`usage.modality` splits the four counts across text, audio and image**; only a realtime voice call reports one. Each part is named after the total it sums into, and a producer that cannot fill every field sends none.

**`usage.service_tier` names the tier that served the call** when it was not the provider's standard one. Today that is only `flex`, OpenAI's Flex processing, which bills every rate at half. Price such a row at its tier's rates.

**`usage.cache_creation_1h_tokens` is the part of `cache_creation_tokens` written to the 1-hour cache.** A 1-hour write bills at 2x base input, a 5-minute one at 1.25x. Claude Code captures carry it when Claude Code split the write by lifetime. Chat calls to Claude on Anthropic or Vertex carry it whenever the reply split the write, where `0` means only 5-minute writes. Absent elsewhere and on older rows.

**`usage.cache_creation_30m_tokens` and `usage.cache_creation_12h_tokens` split an OpenAI Responses write** into its 30-minute and 12-hour parts, which together cover it. A 12-hour write bills at 2.5x input. **`usage.cache_creation_requested_ttl`** (`5m`, `30m`, `1h` or `12h`) names the lifetime the request asked for, where the reply split nothing. Lucidos records it only on a route shown to honour it: today OpenRouter's `anthropic/` models.

The *cache keep-alive*, the cache-warm line and the switch estimate read a thread's cache lifetime from its latest write: the split first, then the requested lifetime. With neither, the lifetime is unknown, never a guess.

A Claude Code sub-agent's call carries `parent_tool_use_id`, the id of the `Agent` call that spawned it. Every other capture omits it.

A Claude Code capture also carries `api_call_id`, the agent's id for that API call. Every `CodingAgentToolCalled` the call produced carries the same value, so group on it to find the tool calls one model call made. Codex captures and older rows omit it.

`session` names the thread's coding-agent session the call ran in, counted from 1. A thread starts in session 1. Each `SessionStarted` whose `continuity.kind` is `fresh` or `handoff` opens the next one, and a resume stays in its session. The event bus stamps it on every capture, so group on it for a thread's cost per session. Context size, cache warmth and the handoff estimate read only the current session. Absent on every row written before the field existed.

A `reconstructed: true` row was rebuilt by the startup backfill from events never captured live. Its `estimated_total_tokens` is a reconstruction and it carries no `usage`, so a rollup filtering on `usage` still reports measured spend only.

`reasoning_effort` names the tier a call ran at (`low`, `medium`, …), where the caller reports one. A chat turn and every metered chat call set it, so a cost estimate can split measured usage by tier instead of one undifferentiated baseline. Absent on a call that names no tier, and on every row written before the field existed.

`served_model` is the model id the provider's reply named, verbatim: Gemini's `modelVersion`, or `model` from OpenAI, Anthropic and OpenRouter. `model` is the one Lucidos asked for. The two differ when another model answered: a provider routed the id elsewhere, or Lucidos sent a retired model's successor. A dated snapshot of the same id (`claude-haiku-4-5-20251001`) is not a reroute. Absent when the reply named no model, on a streamed proxy reply, and on every row written before the field existed.

`provider` names the billing backend proved by the call's route or process (`vertex`, `anthropic`, `claude-gateway`, ...). Coding-agent calls and session-copy pings use `unknown` when their process supplied no proof. Native searches use their serving backend's endpoint and authentication; unproven endpoints omit the field. Older captures can also omit it. Scope a cost rollup with `condition: { provider: "claude-gateway" }`.

`telemetry_owned_by_cli: true` marks an auxiliary capture whose CLI process owns telemetry, such as a session-copy ping. The engine exporter excludes it without changing its producer or billing totals. Absent means false; coding-agent producers retain their own exclusion. Engine-owned prefill pings still export.

On a Claude Code row both fields come from the reply: `model` is the id it named, with the session's `[1m]` put back, and a sub-agent's call names its own model. So the two always read as served as sent, even when the call was set up as `haiku`, `sonnet` or `opus`. A Codex row puts the session's model in `model` and omits this field, since Codex names no model per call.

`duration_ms` is how long a call took, wall clock, where the caller timed it. A chat turn and every metered chat call set it. It spans the provider's retries, so `created` minus `duration_ms` is when the call's first attempt started. A cache-gap measurement needs that: `created` alone is when the reply ended. The backfill estimate also reads it as a model's measured seconds per call at each tier. Absent on a call nobody timed, and on every row written before the field existed.

`ttft_ms` is the time to the first content delta of the stream that answered, from the same start as `duration_ms`, so it never exceeds it. It spans retries too: a call that retried reports the wait for the stream that finally answered. `request_id` is the provider's `request-id` response header for that stream. Both come from the *call trace*, on a chat call that went through a streaming send loop. Both are absent when the stream carried no content or the response had no header. They are also absent on every row written before the fields existed.

A request *held the context* when it read at least half its input from cache. Empty input does not count. A missed keep-alive ping ends its run until a real request starts. A wake claims `saved_usd` only when it held the context while the cache was warm. The claim is zero when the wake came within one cache lifetime of the last real request, since the pings kept nothing warm.

`stable_prefix_fingerprint` is a 16-hex hash of the stable prefix a chat turn's first round sent: the tools, the system block and any memory view blocks, with the model and thinking config, and without cache markers. Claude through Anthropic or Vertex reports it on turn round 1. A prefill capture reports the fingerprint of its actual cut when available. A prefill beat sends the turn's own bytes, key order included, so it reads the turn's cache. Two turns with the same value sent the same stable prefix, so the second could read what the first cached.

`context_window` is the model registry's window, or a fallback from its id:

- Any id containing `[1m]`: 1M.
- `claude-`: 200k.
- GPT major version 5 or later: 400k.
- Other ids: 200k.

A capture showing 200,000 for a bigger model can mean its registry row lacks a
window. The turn was then budgeted against the smaller number.

`estimated_total_tokens` covers the whole request: system prompt, tools and
messages. It estimates tokens at 2.5 characters each. Compare it with
`usage.input_tokens`, the provider's real total, rather than treating it as exact.
The trim budget uses a conservative 1.5 characters per token.

Before 2026-08-07 this readout also used 1.5, making estimates about 1.7 times
higher. Multiply thresholds written for that readout by **3/5**:
`{ $gt: 150000 }` becomes `{ $gt: 90000 }`. Leaving the old threshold can
silently stop a trigger firing.

Each section carries `name`, two sizes, and `role` (`system` / `prior_message` / `user`). It also carries an optional `group` label and an optional `content` body. The body is omitted when the `capture_context` preference is off, and truncated head and tail at 8,000 chars when it is on. The engine reads the preference before each model call, so a turn resumed after a question card follows a switch flipped while it waited.

**The two sizes answer different questions; picking the wrong one is the classic error.**

- `budget_delta_chars` is what the section ADDS to the request beyond what other sections count. Sum this one. The LLM Context Viewer divides the headline total by that sum, so the rows always add up to the number at the top. The headline is the measured `usage.input_tokens` when there is one, else the estimate.
- `content_chars` is the section's own size, measured whether or not the body was persisted. Ask it how big a region was. Never sum it.

On almost every section the two are equal. `Conversation` is the exception: every other section is already in the first message, so its delta is only what the tool loop added. Summing its real size would count the bundle twice.

Both are character counts. No section carries a token count.

**Pre-rename rows spell the delta `char_count`** and carry no `content_chars`. This is a renamed payload key, not a retired event name, so no `Legacy alias:` note applies. `GET /api/v1/events/:event_id/context` renames the key on the way out, but a direct SQL query over history sees it: read `coalesce(x->>'budget_delta_chars', x->>'char_count')`.

`trimmed` means the LLM got less than the assembled context, in **either** way. The trimmer's removal pass evicts whole messages, and its stubbing passes replace a body with a note. The note opens `[cut to fit the context budget:` and states the original size. Where the body had an event address, it also names the `events(action="query", event_id=...)` call that reads it back. Older rows reported only evictions, so stubbed turns showed as untrimmed.

`trim_passes` lists which passes did it, ascending, and is absent when nothing was trimmed. Pass 5 is the one that matters: it alone removes a whole message, while every other pass leaves an addressed stub the model can read back. So `trimmed: true` without pass 5 lost nothing silently. Rows from before the field carry no `trim_passes`, which means unknown, not none.

**Snapshot endpoint strips the heavy fields.** `GET /api/v1/threads/:thread_id/events` removes `sections` and `tools` from `ContextCaptured` and stamps `sections_stripped: true`. One capture can be ~50 kB, and a long session carries hundreds. Live SSE emissions still carry the full arrays.

To fetch them on demand (as the step-detail modal does), call `GET /api/v1/events/:event_id/context`, which returns `{ sections, tools }`. It is keyed on `event_id` only (UUIDs are unguessable, and scope matches the snapshot endpoint). Bulk consumers such as `exportThread.ts` pass `?include_context=true` on the snapshot instead of N+1 fetches. Triggers and subscribers rarely need `sections` / `tools`; if you do, use an on-demand path.

### `ModelCallFailed`

```json
{
  "type": "ModelCallFailed",
  "data": {
    "model": "claude-opus-5-5",
    "provider": "claude-gateway",
    "reasoning_effort": "medium",
    "duration_ms": 31842,
    "attempts": 4,
    "failure": "http",
    "status_code": 529,
    "request_id": "req_011CZ"
  }
}
```

One row per failed metered chat call, on any provider. `purpose` names which call it was, as on a `ContextCaptured`, and is absent for an agent turn. `attempts` counts every HTTP send across the call's retries and model fallback. `duration_ms` spans them too.

`failure` says how it failed, from what the *call trace* saw. `http` is a non-success status, and `status_code` carries it. `connection` and `timeout` mean no response arrived, and `timeout` also covers a caller's own deadline running out. `stream` is a success status followed by a stream that broke. `other` is a failure with none of those facts, such as a credential error before anything was sent. `status_code` and `request_id` are absent when there is none.

**The row holds no error text.** A provider's error body can echo the request, so the message stays with the caller. A turn's `ResponseFailed` keeps its own. Old rows have none of these events, and a call the caller cancels records none.

### Provider spend-cap holds

`ModelSpendCapReached` carries `provider`, `resets_at` and `held_until` for a background-call hold.
`resets_at` is nullable; `held_until` is an engine retry boundary, not a promised provider reset.
`ModelSpendCapHoldEnded` carries the provider and an end cause: `reset`, `succeeded` or `cleared`.
An explicit clear carries its initiating actor.
Hold events persist, but a new engine starts with no active holds.
Settings → Providers shows current holds and offers **Try now** after a cap change.

A `ResponseFailed` can also carry `spend_cap: { provider, resets_at, message }`.
Older events omit it. The frontend displays the provider message and distinguishes an unknown reset time.
A coding-agent notice does not start an engine hold.
`TreeBackfillProgressed.progress` adds `spend_cap_held_until` and `spend_cap_resets_at`, both optional, for Memory's waiting status.

