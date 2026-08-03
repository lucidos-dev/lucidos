# Investigation: `API Error: Stream idle timeout - no chunks received`

Coding-agent sessions in this workspace die with this error, losing all uncommitted
work. This document records the evidence, the mechanism, and the mitigations.

Status: closed. Cause identified, one mitigation shipped, one proposed.

## Answer in one page

- **Who kills it:** the Claude Code CLI, in a byte-level watchdog wrapped around
  the SSE response body. Not Lucidos, not a proxy, not the provider.
- **After how long of what:** **300 seconds with zero bytes on the socket.** Any
  byte resets it, including an SSE `ping`. Measured deaths at 303.1 s and 303.0 s.
- **What produces 300 s of silence:** prompt processing on a large cache-cold
  request. The API sends `message_start` immediately, then says nothing until the
  first content delta. A surviving turn on the same thread wrote 270 176
  cache-creation tokens and took 290 s to its first block.
- **Not the cause:** extended thinking (it streams deltas), `xhigh` effort, engine
  backpressure on stdout (the engine drains it into an unbounded channel), CLI
  version (identical code in 2.1.201 / 2.1.202 / 2.1.220), or machine sleep (that
  raises a different error).
- **Why it is fatal:** CC skips its non-streaming fallback for watchdog errors,
  and its single streaming retry is unavailable once `message_start` has arrived.
- **What Lucidos did wrong:** nothing active, one omission. We never raised the
  deadline, and our own 10-minute inactivity watchdog, whose response is a
  non-destructive auto-resume, is slower than CC's 5-minute one, so the
  destructive handler always won.
- **Shipped:** `CLAUDE_BYTE_STREAM_IDLE_TIMEOUT_MS = 1800000` on every
  coding-agent spawn, which makes the engine watchdog the first responder.
- **Proposed, needs approval:** auto-continue on this error class, bounded to one
  attempt per turn.
- **Upstream:** three closed issues, no fix, no maintainer reply. The watchdog
  itself shipped deliberately in 2.1.196 ("on by default for all providers",
  5 minutes). Nothing to wait for.
- **Version:** `2.1.220` is the newest published build (`npm dist-tags`:
  `latest = next = 2.1.220`, `stable = 2.1.212`), and it is what is installed.
  Already current.

## Evidence

### The failing thread

Thread `235fd311` ("Fixing macOS Update Path Audit Findings") died twice on
2026-08-02. Its Claude Code session transcript survives at
`$CLAUDE_CONFIG_DIR/projects/<escaped-worktree-path>/17a49009-01b8-4132-9c1b-0fb25f398d7a.jsonl`
(102 records), and it is the single most useful artifact in this investigation:
both deaths are recorded in it as synthetic assistant messages.

Timeline reconstructed from the transcript (all times UTC):

| Time | Event | Gap |
|---|---|---|
| 13:25:30 | session starts, first prompt (5 092 chars) | |
| 13:25:32 to 13:26:31 | 15 turns of `Read` / `Bash`, context grows to ~219k tokens | |
| 13:26:31.656 | last tool result (13 727 B) delivered | |
| **13:31:34.738** | **`API Error: Stream idle timeout - no chunks received`** | **+303.1 s** |
| 13:36:38 | user revives the thread | |
| 13:36:52 to 13:37:07 | 5 turns, context ~228k tokens | |
| 13:37:07.983 | last tool result (29 939 B) delivered | |
| **13:42:10.759** | **`API Error: Stream idle timeout - no chunks received`** | **+303.0 s** |
| 13:47:20 | user revives again | |
| 13:52:10.483 | first content block of the reply completes | **+290.1 s** |

The two failures are **303.1 s and 303.0 s** after the last byte of the previous
step. The near-miss that survived is **290.1 s**. Three samples clustered around
300 s is a fixed client-side threshold, not jitter.

### What the surviving turn tells us

The turn that survived (13:47:20 to 13:52:10) reports
`cache_creation_input_tokens = 270 176`, `cache_read_input_tokens = 0`: a total
prompt-cache miss writing 270k tokens. It took 290 s before its first content
block completed, and then produced 22 163 output tokens normally. So the stall is
**time-to-first-token on a very large, cache-cold prompt**, not a hung connection.

The two failing requests sat at ~219k and ~228k tokens of context with a fresh
tool result appended. They needed the same multi-minute prompt-processing pass and
did not get under the wire.

### Provider and configuration in use

Both failing requests carry `requestId: req_vrtx_...`, so the provider was
**Google Vertex AI**, not the personal Anthropic subscription. The spawn env had
`CLAUDE_CONFIG_DIR` pointed at the vertex config dir, whose `settings.json` `env`
block sets `CLAUDE_CODE_USE_VERTEX=1` and pins `claude-opus-5[1m]`. Effort level
`xhigh`. CLI version `2.1.220`, entrypoint `sdk-cli` (Lucidos runs
`--print --output-format stream-json`).

So this is **not** a subscription-vs-vertex artifact in the sense of "one provider
is broken": it is Vertex that stalled, and the client-side deadline that killed it
is 300 s.

## Who kills the stream, and after how long of what

The error string does not exist in Lucidos. It is emitted by the Claude Code CLI
itself. All findings below are read out of the installed `2.1.220` bundle
(a single compiled executable; the JavaScript is greppable inside it).

### The mechanism: a byte-level watchdog on the SSE response body

CC wraps the HTTP response body of any `text/event-stream` response in a
`ReadableStream` that carries a `setTimeout`. The timer is armed in `start()` and
**re-armed after every successful read that yields bytes**. If it expires, the
wrapper errors the stream with a `StreamIdleTimeoutError`
(`stream idle: no bytes for <N>ms`), cancels the reader, and the query generator
converts that into the user-visible failure.

Three properties matter:

- **The reset condition is bytes on the socket, not events and not tokens.** An SSE
  comment or a `ping` frame resets it. Nothing else in the client does.
- **The re-arm happens inside `pull()`.** If the *downstream consumer* stops
  pulling, the timer is never re-armed and fires even though the server is
  streaming. CC distinguishes the two cases in telemetry via `body_read_pending`.
- **Suspend is handled separately.** If the timer fires more than `idleMs / 2`
  late (laptop slept), CC raises `StreamSuspendedError`
  ("Stream watchdog detected system suspend; aborting to retry on a fresh
  connection") instead. That is a different message, so our failures were not
  machine sleep.

Diagnostics it emits: the debug line
`[byte-watchdog] firing: idle=<N>ms late=<N>ms errored=<bool> bodyReadPending=<bool>`
and the telemetry event `tengu_streaming_idle_timeout` carrying `timeout_ms`,
`tier: "byte"`, `bytes_received_before_stall`, `time_to_first_byte_ms`,
`body_read_pending`, `slept_ms`, `cf_ray`.

### The number

The threshold is resolved per request, before any clamp:

```
base    = max(CLAUDE_STREAM_IDLE_TIMEOUT_MS, 300_000)
fallback= provider === "firstParty" ? 180_000 : base
if   CLAUDE_BYTE_STREAM_IDLE_TIMEOUT_MS > 0   -> that value
elif CLAUDE_STREAM_IDLE_TIMEOUT_MS     > 0   -> base
else                                          -> fallback,
                                                 overridable by the remote gate
                                                 `tengu_byte_stream_idle_timeout_ms`
clamp to [10_000, 1_800_000]
```

Constants verified in the bundle: first-party fallback `180000`, clamp floor
`10000`, clamp ceiling `1800000`, base floor `300000`.

For anything that is not first-party the resolved value is **300 000 ms**, and the
observed deaths were at 303.1 s and 303.0 s. That is the number.

Whether the watchdog is installed at all is gated by `CLAUDE_ENABLE_BYTE_WATCHDOG`:
explicitly false (`0`/`false`/`no`/`off`) disables it, explicitly true forces it on,
and otherwise a remote gate (`tengu_stream_watchdog_default_on`) decides, currently
defaulting **on**.

Reading the bundle alone left one open question here: the static provider gate on
the *byte* wrapper arms it for first-party and `anthropicAws` (Bedrock behind an
opt-in flag) and appears to exclude Vertex, yet these Vertex requests were killed
at exactly the non-first-party threshold. **Anthropic's own changelog settles it.**
Release 2.1.196 states that the streaming idle watchdog "is now on by default for
all providers" and that "it aborts and retries when a response stream produces no
events for 5 minutes", with `CLAUDE_ENABLE_STREAM_WATCHDOG=0` as the off switch.
All-providers, 5 minutes, aborts-and-retries-once: that is the vendor describing
the exact behavior measured here, so the narrower gate is not the operative one.

Either way the mitigations below hold: the disable switch and the threshold
override are read unconditionally when the wrapper is constructed, so they take
effect regardless of which branch armed it.

### `no chunks received` vs `partial response received`

At throw time CC picks the wording from how many **content blocks had completed**:

- `partial response received` means at least one content block had finished. CC then
  synthesizes a stop reason, keeps what it has, and appends "Response stalled
  mid-stream. The response above may be incomplete."
- `no chunks received` means **zero content blocks had completed**. The stall
  happened before the model produced anything durable.

Ours is always `no chunks received`, which is exactly what a time-to-first-token
stall looks like: the API sends `message_start` immediately, then goes silent while
it processes a very large cache-cold prompt, and the first `content_block_stop`
never arrives inside the 300 s window.

### Why the built-in retry did not save it

CC has a streaming retry for this error, capped at **one** attempt, via two paths:

1. *"Stream idle timeout before first event"*: requires that **no stream event at
   all** had been forwarded. Since the API sends `message_start` promptly, this path
   is closed for a mid-prompt-processing stall.
2. *"Stream idle timeout after thinking-only yield"*: requires that only thinking
   blocks exist so far. Also unavailable when nothing has completed.

And the non-streaming fallback is deliberately skipped for watchdog errors
(`tengu_watchdog_skip_nonstreaming_fallback`, default true), so CC does not retry
the request unstreamed either. The failure is terminal for the turn.

For completeness, two knobs that sound relevant and are not:

- `API_TIMEOUT_MS` (default 300 000 in this code path) is passed as the SDK
  per-request `timeout`. The SDK implements it as a `setTimeout` cleared in a
  `finally` once the fetch resolves, so for a streaming request it bounds
  time-to-headers only, never the body. It cannot produce this error.
- `CLAUDE_CODE_RETRY_WATCHDOG` reads like a watchdog knob but is a different thing.
  In the bundle it gates the **outer request-retry layer**: it lifts the 15-retry
  clamp on `CLAUDE_CODE_MAX_RETRIES` and keeps retrying overloaded / 429 responses
  past the normal caps. Anthropic describes it as raising "the default retry count
  for non-capacity transient errors to 300" (2.1.199) and recommends it "for
  unattended sessions" (2.1.186), which Lucidos coding-agent sessions are. It is
  therefore worth considering separately, but it does **not** rescue this error:
  the stream-idle failure is thrown as a plain `Error` with no HTTP status, so the
  retry classifier this flag widens never matches it. Treat it as a candidate for
  overload resilience, not as a fix for this.

## Is the trigger a long single assistant turn

Yes, but not in the way the brief guessed. It is not extended thinking producing a
silent stretch: thinking streams as deltas, and every delta is bytes that reset the
timer. It is **prompt processing on a large, cache-cold request**, which is silent
on the wire from `message_start` until the first content delta.

The transcript supports this directly. The turn that survived wrote 270 176
cache-creation tokens with zero cache reads and took 290 s to its first completed
block. The two that died sat at ~219k and ~228k tokens of context with a fresh
tool result appended, needing the same pass, and crossed 300 s.

So the real risk factor is **prompt size at a cache boundary**, and the brief's
reading-heavy shape is a proxy for it: reading a 1 600-line script and a 1 070-line
report front-loads context to ~200k tokens fast, and every subsequent turn that
misses the cache pays a multi-minute silent prompt-processing pass.

Corollary that matches the sibling evidence: the 43-minute session that finished
cleanly the same day did many small steps against a warm cache, so it never had a
single 300 s silent window. "Long sessions die" was never the rule. "Sessions that
build a huge context and then miss the cache die" is.

## Does anything on the Lucidos side make it worse

### Cleared: we do not starve the CLI's stdout

The obvious way a host process can cause this error is backpressure: the byte
watchdog re-arms inside `pull()`, so a consumer that stops reading trips it even
while the server streams. Lucidos does not do that.

`runtime/claude_code.rs::driver_task` reads the child's stdout line by line inside
a `tokio::select!` and forwards every parsed event into an **unbounded**
`mpsc::UnboundedSender`. `send` on an unbounded channel never awaits, so the read
loop is never blocked by a slow consumer, and the pipe is drained as fast as the
child writes. The only moments the loop is not reading stdout are the two write
arms (`stdin.write_all` + `flush` for a user message or a control request), which
are sub-millisecond pipe writes.

So the `body_read_pending = false` failure shape cannot originate here. Whatever
this was, it was silence on the wire, not silence in our reader.

### Cleared: `--include-partial-messages` and `xhigh` are not the trigger

`build_command` always passes `--include-partial-messages`. That flag is what feeds
`AgentEvent::StreamActivity` and keeps the *engine's* inactivity watchdog fresh
through a long step. It has no effect on CC's byte watchdog, which counts bytes on
the HTTP socket, one layer below.

`CLAUDE_CODE_EFFORT_LEVEL=xhigh` lengthens extended thinking. Thinking streams as
`thinking_delta` frames, and every frame is bytes that reset the watchdog, so a
long think is not a silent stretch. What `xhigh` does do is grow output, which
grows context faster, which brings the prompt-size risk factor forward. Indirect,
not causal.

### The real contribution: we never raise the deadline, and our watchdog is slower

Two facts combine into the whole failure mode.

1. **`build_command` sets no streaming or timeout env at all.** It sets
   `MCP_TOOL_TIMEOUT` and `MCP_TIMEOUT` to 24 hours (deliberately, for the
   permission-prompt flow), `CLAUDE_CODE_EFFORT_LEVEL`, and `CLAUDE_CONFIG_DIR` on
   resume. Nothing touches `CLAUDE_BYTE_STREAM_IDLE_TIMEOUT_MS` or
   `CLAUDE_STREAM_IDLE_TIMEOUT_MS`, so every coding-agent session runs on the stock
   300 s deadline even though Lucidos routinely drives CC to 200k+ token contexts
   where the provider needs longer than that.

2. **The engine's own inactivity watchdog fires at 10 minutes, CC's at 5.**
   `WATCHDOG_INACTIVITY_LIMIT_MS = 10 * 60 * 1000`, and when it fires the action is
   `SafetyNetAction::EmitContinuationRequested`, i.e. kill the subprocess and
   **auto-resume via `--resume`**. That is exactly the recovery this failure wants.
   It never gets to run, because CC's 300 s deadline always wins the race and ends
   the turn with a terminal `ResponseFailed` instead.

So Lucidos already owns a non-destructive recovery for "the subprocess went silent
mid-turn", and the only reason it does not cover this case is ordering: the
component with the destructive response has the shorter fuse.

### Consequence for the user

`classify_result` maps a `Result` carrying `cc_error` straight to
`TerminalKind::Failed { error }`, and `should_auto_commit_on_cleanup` only
auto-commits on `Generated`. So a stream-idle death:

- emits `ResponseFailed` and parks the thread waiting for a human,
- does **not** auto-commit the worktree,
- and leaves whatever the agent had written sitting uncommitted in the worktree
  (Continue resumes into the same worktree, so files are not destroyed; the lost
  thing is the turn and everything the agent had reasoned but not yet written).

### The revive path is the riskiest turn in a session

A revive resumes with `--resume`, which replays the whole conversation. The first
turn after a revive is therefore the one most likely to be a full prompt-cache
write, and thus the one most likely to stall. The transcript shows it: the first
turn after the second revive took 290.1 s to its first completed block, 10 seconds
inside a deadline that had already killed the thread twice.

## Upstream status

### Are we on the latest CLI? Yes, and ahead of the stable channel.

Checked deterministically rather than from prose: `npm view @anthropic-ai/claude-code dist-tags`
returns `latest = 2.1.220`, `next = 2.1.220`, `stable = 2.1.212`. The installed
CLI is **2.1.220**, so it is the newest published build and one channel *ahead* of
`stable`. There is no newer version to upgrade to, and no reason to move to
`stable` (2.1.212 predates two fixes that matter to how Lucidos drives CC:
2.1.214's "stream-json output truncation at exit for slow-reading SDK/pipeline
consumers" and 2.1.219's "`claude -p` text output dropping the answer already
produced when a turn dies on a mid-stream API error").

### Where the watchdog came from

Release **2.1.196**: the streaming idle watchdog "is now on by default for all
providers", aborting and retrying "when a response stream produces no events for
5 minutes", with `CLAUDE_ENABLE_STREAM_WATCHDOG=0` to turn it off. That is the
change that created this failure mode, and nothing in 2.1.201 through 2.1.220
touches it: across those twenty releases the changelog has no watchdog or
stream-idle entry at all.

### Is 2.1.220 a regression? No.

All three CLI versions present on this machine were decompiled and compared. The
byte watchdog, its threshold resolution, and both env overrides are **identical**
in `2.1.201`, `2.1.202` and `2.1.220`: the same
`max(CLAUDE_STREAM_IDLE_TIMEOUT_MS, 300000)` base, the same `180000` first-party
fallback, the same `CLAUDE_BYTE_STREAM_IDLE_TIMEOUT_MS` direct override, the same
remote-gate hook, and the same "before first event" / "after thinking-only yield"
retry messages.

**Downgrading to 2.1.202 or 2.1.201 would change nothing.** Do not try it.

### Is there a known upstream issue? Several, all closed without a fix.

- [#49716](https://github.com/anthropics/claude-code/issues/49716) is the closest
  match to our shape: a **pre-stream** stall where the server sends zero bytes,
  while immediately-preceding requests in the same session had sub-1.5 s
  time-to-first-byte. Filed on 2.1.112 against the first-party API with Opus at
  `effort = xhigh`. Its debug log shows CC's own
  `Slow first byte: no stream chunk 30.0s after request sent (attempt 1)` warning
  followed five minutes later by the abort. **Closed as duplicate, no maintainer
  reply, no fix, no workaround, no env var offered.**
- [#53730](https://github.com/anthropics/claude-code/issues/53730) reports the
  `partial response received` variant, 10 to 15 times per session, and its headline
  request is exactly ours: *"the error itself may be inevitable at scale, but
  failure without automatic repeated attempts to continue is the part that makes it
  a problem."* **Closed as not planned.**
- [#25979](https://github.com/anthropics/claude-code/issues/25979) (tagged
  `api:vertex`) and [#33949](https://github.com/anthropics/claude-code/issues/33949)
  cover the opposite failure, a stall with **no** timeout at all. The byte watchdog
  is the fix that was shipped for those, which is why it is aggressive.

So: the deadline is deliberate, the aggressiveness is deliberate, and the missing
automatic retry is a known, declined request. There is no upstream fix to wait for.

### The env knobs, named exactly

Read out of the 2.1.220 bundle, not from a blog post. All are read from the process
environment, so Lucidos can set them on the spawn.

| Variable | Effect |
|---|---|
| `CLAUDE_BYTE_STREAM_IDLE_TIMEOUT_MS` | Sets the byte-watchdog threshold directly. Clamped to `[10000, 1800000]`. Highest precedence. |
| `CLAUDE_STREAM_IDLE_TIMEOUT_MS` | Sets the base, floored at `300000`; also suppresses the remote-gate override. Cannot lower the deadline below 5 minutes. |
| `CLAUDE_ENABLE_BYTE_WATCHDOG` | `0`/`false`/`no`/`off` removes the watchdog entirely; truthy forces it on; unset defers to a remote gate that currently defaults on. |
| `CLAUDE_SLOW_FIRST_BYTE_MS` | Threshold for the advisory `Slow first byte` warning only. Does not abort anything. |
| `CLAUDE_CODE_DISABLE_NONSTREAMING_FALLBACK` | Unhelpful here: the non-streaming fallback is already skipped for watchdog errors. |

The ceiling matters: **30 minutes is the maximum**, `CLAUDE_BYTE_STREAM_IDLE_TIMEOUT_MS`
is clamped and cannot be set to infinity. Fully disabling the watchdog is only
possible via `CLAUDE_ENABLE_BYTE_WATCHDOG=0`, which reintroduces the hang that
#25979 and #33949 describe.

Sources:
[#49716](https://github.com/anthropics/claude-code/issues/49716),
[#53730](https://github.com/anthropics/claude-code/issues/53730),
[#25979](https://github.com/anthropics/claude-code/issues/25979),
[#33949](https://github.com/anthropics/claude-code/issues/33949).

## Can we survive it instead of failing the thread

Five options were evaluated against the code. Two are worth doing, three are not.

### A. Raise the client deadline on the spawn (done, this change)

`build_command` sets `CLAUDE_BYTE_STREAM_IDLE_TIMEOUT_MS` to `1800000` (the clamp
maximum, 30 minutes) **before** `apply_lucidos_env`, so a workspace env var of the
same name still wins and the value stays tunable without a rebuild.

The point is not "30 minutes is the right deadline". The point is the ordering.
Lucidos already owns a watchdog for "the subprocess went silent mid-turn":
`WATCHDOG_INACTIVITY_LIMIT_MS` at 10 minutes, whose action is
`SafetyNetAction::EmitContinuationRequested`, i.e. kill the subprocess and
**auto-resume the turn via `--resume`**. That is precisely the recovery this
failure needs, it is already written and tested, and the only reason it never runs
is that CC's 300 s deadline fires first with a destructive response.

Moving CC's deadline past ours inverts that:

| Stall length | Before | After |
|---|---|---|
| under 5 min | completes | completes |
| 5 to 10 min | **thread fails**, user must Continue | completes |
| over 10 min | **thread fails**, user must Continue | engine watchdog kills and auto-resumes |

Two rejected variants of the same idea:

- **`CLAUDE_ENABLE_BYTE_WATCHDOG=0`** would remove the deadline entirely. That
  reintroduces the unbounded hang of #25979 / #33949 for the cases where the
  engine watchdog legitimately stands down, notably `tools_in_flight > 0` below
  the 45-minute `WATCHDOG_HUNG_TOOL_CEILING_MS`. Keep a backstop; just make it the
  outer one.
- **`CLAUDE_STREAM_IDLE_TIMEOUT_MS`** also works but is the blunter knob: it is
  floored at 300 000, it feeds other call sites, and it suppresses the remote-gate
  path. `CLAUDE_BYTE_STREAM_IDLE_TIMEOUT_MS` is the direct, highest-precedence
  override for exactly the timer that fired.

### B. Treat the error class as recoverable and auto-continue (proposed, not done)

Option A removes the trigger. This is the belt for when it fires anyway, and it is
the thing issue #53730 asked upstream for and did not get.

Today `classify_result` maps **any** `cc_error` to `TerminalKind::Failed { error }`,
which emits `ResponseFailed` and parks the thread waiting for a human. The proposal
is to carve out a narrow transient class and auto-continue instead:

- Add a predicate beside `is_definitive_session_not_found` in
  `agent_session/lifecycle.rs` matching CC's stream-idle wording. That file already
  carries exactly this shape of CC error-string tolerance, with a registry entry
  saying to switch to a structured signal when CC offers one.
- On a match, emit `ContinuationRequested` after the `ResponseFailed` rather than
  going idle, reusing the same auto-recovery variant the watchdog already emits.
  The session id is still valid and the conversation is intact, so `--resume`
  picks up exactly where it stopped. The transcript shows both manual revives
  worked, one in 14 s.
- **Bound it.** One automatic continuation per turn, tracked the way
  `agent_recovery::helpers::continue_recovery` already tracks its one-shot
  `retried` flag, so a provider having a bad hour cannot loop the session and burn
  quota. Past the bound, fail as today.

Touch points: `agent_session/lifecycle.rs` (predicate plus the decision function),
`agent_session/run_session/run.rs` (the `AgentEvent::Result` arm),
`agent_recovery/helpers.rs` (the bound), `lifecycle_tests/classify.rs` (tests).

Not implemented here because it changes the thread lifecycle: a turn that today
ends and waits would start re-spawning work on its own. That is an implementation
plan and a human approval, not a drive-by.

### C. An engine-side keepalive (rejected, not possible)

There is nothing to send. The watchdog counts bytes arriving on **CC's own HTTPS
socket to the provider**. Lucidos sits on the other side of CC, on stdin and
stdout. No byte we write can reach that socket, and no flag asks CC to send one.
A keepalive would have to come from the provider, as an SSE `ping`.

### D. Downgrade the CLI (rejected, would not help)

Verified above: identical watchdog, identical constants in 2.1.201, 2.1.202 and
2.1.220.

### E. Keep the prompt smaller (real, but user-side)

The risk factor is prompt size at a cache boundary, so anything that keeps a
session's context off the 200k mark lowers the odds: delegate bulk reading to a
subagent whose context is discarded, read the parts of a large file that matter
rather than the whole thing, and commit early so a lost turn costs less. Worth
saying in knowhow; not something the engine can enforce.

### What is NOT a mitigation

Committing early does not prevent the failure, it only limits the damage. The
brief's instruction to commit after every section is correct practice and is what
made this document survivable, but it is triage, not a fix.

## What shipped, and where the rest lives

**Code.** `runtime/claude_code.rs` gains `CC_BYTE_STREAM_IDLE_TIMEOUT_MS` and
writes `CLAUDE_BYTE_STREAM_IDLE_TIMEOUT_MS` on every coding-agent spawn, before
`apply_lucidos_env` so a workspace env var still overrides it.
`agent_session::lifecycle::WATCHDOG_INACTIVITY_LIMIT_MS` became `pub(crate)` so
the ordering invariant ("CC's deadline must exceed the engine's") is asserted by a
test in `runtime/claude_code_tests/build_command.rs` instead of being described in
a comment that nothing checks.

**Registry.** `docs/temporary-measures.md` § 1 carries
"CC byte-idle deadline raised past the engine watchdog" with the removal
condition: drop it when CC either auto-resumes this error class or defaults above
our watchdog.

**Knowhow.** `knowhow/claude-code-stream-idle-timeout.md` in this workspace holds
the operational version: what the error means, what to do when a thread hits it
(Continue works), and the three things not to try (downgrade the CLI, disable the
watchdog, or blame the length of the session).

**Open.** Mitigation B above (bounded auto-continue on a transient stream error)
is specified but not built. It needs an implementation plan and approval because
it changes the thread lifecycle.

## How to verify a recurrence is this and not something else

The Claude Code session transcript outlives the thread. It is at
`$CLAUDE_CONFIG_DIR/projects/<escaped-worktree-path>/<session-id>.jsonl`, and a
watchdog death appears in it as an assistant record with `"model": "<synthetic>"`,
`isApiErrorMessage: true`, and the error string as its only text. Two checks
settle it:

1. Subtract the previous record's timestamp. Roughly 300 s (or roughly whatever
   `CLAUDE_BYTE_STREAM_IDLE_TIMEOUT_MS` is set to) means the watchdog. Anything
   else means look elsewhere.
2. Read the `requestId` prefix for the provider (`req_vrtx_` is Vertex), and read
   `cache_creation_input_tokens` on the nearest successful turn for how cold the
   prompt was.

Running CC with `--debug` also surfaces
`[byte-watchdog] firing: idle=<N>ms late=<N>ms errored=<bool> bodyReadPending=<bool>`
on stderr. `bodyReadPending: false` there would mean the consumer stalled rather
than the server, which would point back at the host process. It was not that here.
