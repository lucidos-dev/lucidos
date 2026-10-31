# 0547: The keep-alive beats a Claude Code session parked on a card by asking a session copy, and a cache_keep_alive_mode preference picks the give-up rule; amends ADR 0470's rejected alternative and 0489's single rule

- **Status**: Accepted
- **Date**: 2026-10-22
- **Amends**: [0470](0470-keep-the-prompt-cache-warm.md). It rejected
  keep-alives on question cards and permission prompts, because the session
  blocks mid-turn. A copy of the session can still send the request.
- **Amends**: [0489](0489-keep-alive-budget-follows-model-prices.md). Break-even
  stays the default for event waits. A parked wait gives up at half of it, and
  a preference can pick another rule or turn the keep-alive off.
- **Amended by**: [0554](0554-keep-alive-per-thread-switch.md). A thread can turn the keep-alive off for one wait.
- **Amended by**: [0556](0556-lucidos-agent-keep-alive-prefill-beats.md). A Lucidos Agent thread parked on its own card is kept warm by a prefill beat.
- **Amended by**: [0563](0563-cache-keep-alive-speaks-one-grammar.md). A parked wait shows its status and card, not only the cache-warm line.
- **Amended by**: [0603](0603-keep-alive-follows-the-expected-wake.md). The `workspace-data` rule is retired; the mode is off or on.

## Context

In the keep-alive's first week, Claude Code rewrite cost per 1k calls fell 89%.
Question-card rewrites rose instead: 119 of 201 rewrites, and $81 of $137.

Of those 119, 118 were a live session blocked in the `AskUserQuestion`
PreToolUse hook. The answer landed, the tool result followed within a second,
and the next call of the same process rewrote. The prefix had not changed. The
cache had expired: the median gap from the last request was 750 s.

A *side question* already asks a *session copy*: the thread's own `claude`
command, resumed with `--no-session-persistence` under a deny-all hook (ADR
0324). It reads the parent's cached prefix, and its read refreshes the entry.
The events table holds a natural experiment: two side questions during a card's
wait, then an answer 424 s after the last request that still read its prefix
from cache.

## Decision

- **A parked session is beaten by a session copy.** It is a Claude Code thread
  in `waiting_for_user_answer` whose live session holds the thread, parked on
  a question card or a permission prompt. The keep-alive asks a copy of the
  session for one word, at most one model turn, and kills it after 60 s. The
  parked session, its transcript and its card never hear of it.
- **The copy's spend is a `ContextCaptured`** with purpose `cache_keep_alive`
  and its wall-clock `duration_ms`. The next beat times from the later of the
  last turn request and the last copy.
- **No copy without the remembered appended prompt.** After a restart or an
  eviction, a copy would miss the cache and pay a full write, so the thread
  gets no copy beat.
- **The `cache_keep_alive_mode` preference picks the give-up rule**, for every
  keep-alive wait:
  - `default`: break-even for event waits and working children, half of it
    for a parked wait.
  - `workspace-data`: per wait type, the cap that would have cost least over
    this workspace's last 30 days. Fewer than 50 waits keeps the default.
  - `off`: no beats and no status.

## Rationale

- **Human answers thin out with time.** Over a week of 566 answered cards: p50
  124 s, p90 774 s, p95 37 minutes. Simulated at each card's own context and
  model, half break-even (~48 min on Opus 5.5) saved 51% of the rewrite cost.
  Full break-even saved 43%, and never stopping cost more than doing nothing.
- **Half break-even keeps the worst case bounded.** A card nobody answers pays
  the beats and then the rewrite, 1.5 rewrites at most.
- **The copy shares the side question's spawn.** Same flags, model, effort,
  tools and appended prompt, so the prefix matches by construction.
- **One max turn caps a stray tool call.** The copy's hook refuses every tool.
  A second turn would read the whole context again for nothing.
- **A tuned cap is measured, not guessed.** The estimator counts in reads, so
  one sorted duration list serves every model and TTL. It never goes past
  twice the break-even budget, so every rule stays bounded.

## Consequences

- **A parked thread still shows no keep-alive status.** ADR 0540's card would
  open an exchange beside a live question card. A parked session drops any
  status it held. The copies show in Token Cost under their own purpose.
- **A parent whose children all park keeps no keep-alive.** It wakes only when
  a child finishes, long after the answer.
- **The Lucidos Agent's parked loops get nothing.** They are not Claude Code;
  their cache is the engine's own request.
- **A dead session gets nothing.** A park preserved across a restart has no
  live process and no remembered prompt.
- **Codex gets nothing**, as in ADR 0470.
- **The cache-warm line and the switch estimate read the copy beats.** They
  share one definition of the last cache refresh with the keep-alive. The line
  says `kept until HH:MM`, the spend so far, and why nothing keeps a thread
  warm when that is so.

## Alternatives considered

- **Resume the parked session.** A new turn would supersede the card, and the
  session is blocked inside the hook anyway.
- **The 1-hour TTL for parked sessions only.** The TTL is per spawn, not per
  wait, so it raises the write price of every call. ADR 0470 holds.
- **Full break-even for parked waits.** It saves 43% against 51%: beats past
  an hour mostly pay for cards nobody answers.
- **Show the estimated caps in Settings.** The status card already shows each
  run's give-up time.

Plan, measurements and invariants:
`docs/plans/2026-10-22-keep-cache-warm-on-question-cards.md`.
