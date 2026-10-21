# 0470: Keep the Claude Code prompt cache warm with short waits and a keep-alive; the 1-hour TTL is rejected

- **Status**: Accepted
- **Date**: 2026-10-13
- **Amends**: [0257](0257-coding-agents-wait-through-engine-owned-background-tasks.md).
  Its "never in-turn" held because an in-turn wait blocked past the cache.
  A polled wait returns inside it, so long work now waits in-turn by default.
- **Amended by**: [0479: The keep-alive follows the observed cache TTL](0479-keep-alive-follows-the-observed-cache-ttl.md).
  Its 4-minute beat holds for a 5-minute cache only.
- **Amended by**: [0489: The keep-alive gives up at break-even](0489-keep-alive-budget-follows-model-prices.md).
  Its fixed 150-minute cut-off holds for one model only.
- **Amended by**: [0492](0492-keep-alive-beat-lands-near-4-5-minutes.md). The
  5-minute beat fires at 230 s, so its request lands near 4.5 minutes.
- **Amended by**: [0529](0529-keep-alive-shows-one-status.md). Each ping still
  leaves no trace, but one aggregate status now shows the keep-alive.

## Context

The Claude Code prompt cache lives 5 minutes from the start of the last
request. A cache write costs 25 times a cache read, so one re-write of a large
context costs as much as 25 warm calls.

We measured a month of `ContextCaptured` rows in a busy development workspace.
Re-writes after a gap over 5 minutes were over a quarter of all Claude Code
spend. Two causes dominated:

- foreground tool calls that blocked past 5 minutes, under the prescribed
  `timeout: 600000` and `/harden`'s 480 s suite join;
- event waits that woke the agent long after its cache expired.

A resume that starts within 5 minutes reads 97 to 99 percent of its context
from cache. Past 5 minutes, almost every resume misses.

## Decision

Keep the cache warm rather than lengthen it:

- **No blocking call outlasts the cache.** A wait returns within 230 s, and a
  foreground call is capped at 240 s, which leaves room for its own output.
  `/harden`'s suite join returns within 230 s and starts the Codex driver
  tests in the background.
  A long command runs as a *polled background task*: `lucidos background-task
  run --poll`, then `wait <id>` until it finishes. Only an open-ended wait,
  such as a run queued behind a held lock, keeps the watched task and ends
  the turn. The engine kills every background task after an hour at most.
- **The engine runs a *cache keep-alive*.** It applies while an idle Claude
  Code session waits on a keep-warm event wait or on a working child thread.
  The engine then resumes it about every 4 minutes with a hidden nudge. It
  gives up 150 minutes after the wait began. The turn leaves no visible trace.

## Rationale

- **The cap sits under 300 s with margin.** The lifetime runs from the start of
  the previous request, and that request's own generation time counts against
  it. 230 s plus a few seconds of turn overhead lands well inside.
- **A keep-alive costs one warm call per beat.** Against measured wait lengths,
  keeping a wait warm up to 150 minutes cost 30 percent of doing nothing in
  re-write units. Never stopping cost 31 percent, and the cut-off barely moves
  the result, so it bounds a stuck wait cheaply.
- **Only waits that usually end soon qualify.** The keep-warm kinds are the
  background task, the e2e and suite locks, child completion and idle, and
  change proposed and applied. Other waits run for hours, where beats would
  cost more than the one re-write they save.
- **The keep-alive reuses the wait re-entry route.** Same model, effort, spawn
  arguments and system prompt, so the cached prefix holds.

## Consequences

- **Phase 1 changes what every coding agent is told.** The engine prompt,
  `CLAUDE.md` and the `run-tests` and `run-e2e` skills prescribe three shapes:
  a foreground call under 4 minutes, a polled task, a watched task.
- **A keep-alive turn needs its own handling everywhere a turn has effects.**
  `thread_summaries.keep_alive_turn` keeps it off the summary row. Waits,
  triggers and the parent fan-in skip its idle, which carries
  `reason: "cache_keep_alive"`. The run session proposes nothing and forces no
  read decision, and the transcript drops it. A new turn effect must check it.
- **A child parked on the user keeps no parent warm.** A child waiting on a
  question, permission or plan approval card has no bound on its wait. So the
  child trigger counts only `running` children. An answer restarts the beats
  only while the parent's last request started under 260 s ago. Later than
  that, the cache has expired, and the final wake pays the one re-write.
  Observed live: six nudges on a 400k-token parent while its planning child
  waited on the user.
- **Codex sessions get no keep-alive.** Their cache is OpenAI's and behaves
  differently. They inherit the shorter waits, which only cost extra calls.
- **The savings scale with the number of agent threads**, and the measurement
  in the plan checks them after a week.

## Alternatives considered

- **The 1-hour TTL** (`CLAUDE_CODE_PROMPT_CACHE_TTL=1h`). Rejected. Every cache
  write would cost 2 times the base input price instead of 1.25 times, on every
  call. In simulation against the same month of calls, it also saved less than
  the two phases together.
- **The 1-hour TTL plus the keep-alive.** Rejected: the keep-alive already
  covers the waits the longer TTL would, so the higher write price buys
  nothing.
- **Keep-alives on question cards and permission prompts.** Rejected: Claude
  Code blocks mid-turn inside the hook, so no request can be sent.
- **Keep-alives on every idle thread.** Rejected: most idles never get a reply,
  so beats there would cost several times what they save.

Plan, measurements and invariants:
`docs/plans/2026-10-13-keep-claude-code-cache-warm-during-waits.md`.
