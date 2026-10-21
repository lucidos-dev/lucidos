# 0511: A subagent is a blocking call: it counts against the cache, whose lifetime the session context states from the observed TTL

- **Status**: Accepted
- **Date**: 2026-10-17
- **Amends**: [0470](0470-keep-the-prompt-cache-warm.md). Its 4-minute cap
  named one lifetime and left subagents out.

## Context

A foreground `Agent` call blocks the parent Claude Code session mid-turn. The
keep-alive of ADR 0470 beats only an idle session, so it cannot reach one. A
subagent that outlives the cache costs the parent a full context rewrite.

We measured 30 days of `Agent` calls in a busy development workspace, one row
per parent request:

| Subagent wait | Parent requests | Context rewritten |
|---|---|---|
| under 4 min | 2,668 | 3% |
| 4 to 5 min | 247 | 8% |
| 5 min or more | 748 | 73% |

The long waits cost about 3.3% of Claude Code spend once ADR 0470's fixes are
counted. Ad hoc subagents made up two fifths of it, in any repo. So repo
skills alone could not fix it: only the engine prompt reaches every session.

The engine prompt also named 5 minutes as the lifetime. ADR 0479 showed the
lifetime depends on billing and settings, and is 1 hour on a subscription.

## Decision

- **The static rule counts a subagent as a blocking call.** Size each one to
  finish inside the cap, since a fan-out lasts as long as its slowest agent.
  Longer delegated work goes to a child thread, and the agent ends its turn.
- **The static rule names no lifetime.** It points at the session context.
- **The session context states the lifetime and the cap**: 5 minutes and 4,
  or 1 hour and 50. The values live on `CacheTtl` (`engine/cache_ttl.rs`).
- **The lifetime is the repo's latest Claude Code cache write**, of a
  session's own, among the repo's threads in the last 7 days.
  `CacheTtl::observed` reads it, the function the keep-alive uses. Unknown,
  and every Codex session, state 5 minutes.

## Rationale

- **A child thread is already a keep-warm wait.** The parent ends its turn,
  the keep-alive beats it, and the child's report wakes it.
- **The session context keeps the prompt cacheable.** ADR 0491 moved every
  per-session value out of the appended prompt, so two sessions share its
  cache entry. A lifetime in the prompt would split that entry by billing.
- **The repo is the closest evidence for a fresh session.** A new session
  has no write of its own. Billing is shared across a workspace, but a repo's
  project settings can set `promptCacheTtl`, so another repo's write could
  mislead.
- **A resume and a Codex session skip the read.** A resume drops the session
  context, and Codex states 5 minutes whatever was observed.
- **Unknown states 5 minutes**, for ADR 0479's reason: a short cap on a long
  cache costs extra calls, a long cap on a short cache loses it.
- **The 50-minute cap matches the keep-alive's 1-hour beat**, which fires at
  55 minutes. It leaves room for the call's output and turn overhead.

## Consequences

- **Every flavor gets it, external repos included.** It rides the shared
  prompt base and `spawn_prompts`.
- **The fixed shapes stay.** The 240 s foreground timeout and the 230 s
  polled wait are right under both lifetimes. A 1-hour session makes a few
  extra wait calls.
- **The statement is made once per session.** A TTL that changes mid-session
  is rare, and a resume keeps the earlier statement.
- **Repo skills are unchanged.** `code-review`, the `/harden` suites join and
  `/harden-project` still fan out as before. The re-measurement in the plan
  decides whether they need their own fix.

## Alternatives considered

- **Time-box the repo skills' fan-outs.** Deferred by the user. It covers
  three fifths of the cost and none of the ad hoc subagents in other repos.
- **Keep question cards warm by ending the turn after 230 s.** Declined in
  the same design thread: about 1.6% of spend, for a protocol change.
- **State the lifetime in the appended prompt.** Rejected: it would split the
  prompt's cache entry by billing, against ADR 0491.
- **A TTL-aware `background-task wait`.** A 1-hour session's wait could return
  within about 570 s, under Claude Code's 10-minute Bash ceiling. Not chosen
  for now: it saves only extra reads, and only on subscriptions.
- **Re-enable Claude Code background mode.** Rejected by ADR 0358.

Plan and invariants: `docs/plans/2026-10-17-subagent-waits-stay-inside-the-cache.md`.
