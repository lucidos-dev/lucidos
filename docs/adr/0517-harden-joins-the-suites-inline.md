# 0517: /harden joins the suites inline: a subagent join outlives the cache and rewrites the parent's context

- **Status**: Accepted
- **Date**: 2026-10-18
- **Amends**: [0511](0511-subagent-waits-follow-the-observed-cache-ttl.md). Its
  "repo skills are unchanged" consequence no longer holds for the suites join.

## Context

`/harden` Phase 4.5 joined the test suites in one `Agent` subagent on Claude
Code. Decision 4 of
`docs/plans/2026-10-16-automatic-handoff-checkpoints-and-build-chain.md` added
it, to spare the parent a full context read on every 230 s `wait`.

A foreground subagent blocks the parent mid-turn, and the keep-alive
(ADR 0470) cannot beat a session that is mid-turn. ADR 0511 counted subagents
against the cache, but left the repo skills as they were.

Measured in the dev workspace in the first day after ADR 0511 landed:

| Subagent | Calls | 5 min or more | Median | Max |
|---|---|---|---|---|
| `/harden` suites join | 14 | 14 | 17.5 min | 38.6 min |
| `/harden` Phase 1 review | 11 | 3 | 1.7 min | 6.2 min |
| Ad hoc | 3 | 0 | 1.3 min | 1.6 min |

After each suites join, the parent's next call wrote about 174k tokens to the
cache and read about 8k. Every join rewrote the parent's whole context.

## Decision

On both backends, the parent runs `./scripts/harden-suites.sh wait` itself, in
the foreground with `timeout: 240000`, and re-issues it on exit 3. No subagent
joins the suites.

## Rationale

- **A read is a tenth of a write's price.** On a 5-minute cache, with a parent
  context of C tokens, a subagent join that outlives the cache costs one write
  of 1.25 C. An inline join costs 0.1 C per `wait`. The median 17.5-minute join
  is about 5 calls, 0.5 C.
- **Inline wins up to about 12 calls**, roughly 45 minutes. That is above the
  measured maximum.
- **One shape for both lifetimes.** On a 1-hour cache the subagent would cost
  nothing extra, and inline costs a few reads. ADR 0511 accepted the same trade
  for every other wait.
- **Codex already joined inline**, so both backends now follow one paragraph.

## Consequences

- **The parent's main-session call count grows** by about one per 4 minutes of
  suite run. The handoff estimate reads the measured harden median, so it
  adapts without a constant change.
- **Fix diagnosis lands in the parent's context**, as it did before the
  subagent. That is the ordinary Phase 4 path.
- **The Phase 1 review subagent stays.** Its median is under 2 minutes. A
  re-measurement decides whether it needs a cap of its own.

## Alternatives considered

- **Keep the subagent.** Rejected: it rewrote the parent's context after every
  join measured.
- **End the turn during the join** and let the keep-alive cover it. Rejected
  for now: Apply reads the next idle as the end of `/harden`, so this needs an
  engine change to that contract.
- **Choose by the stated cache lifetime**: inline on 5 minutes, subagent on 1
  hour. Rejected: a second branch in the skill to save a few cache reads.

Plan: `docs/plans/2026-10-18-harden-joins-the-suites-inline.md`.
