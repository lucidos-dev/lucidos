# 0603: Keep-alive follows the expected wake: runs split at a real turn, the reread cap prices every plan, the mode is off or on

- **Status**: Accepted
- **Date**: 2026-10-30
- **Amends**: [0489](0489-keep-alive-budget-follows-model-prices.md). Its
  break-even budget survives as the *reread cap*, which now prices every plan
  and is the fallback with no estimate.
- **Amends**: [0547](0547-keep-alive-beats-a-parked-session-by-copy.md). Its
  `workspace-data` give-up rule is retired, and the mode is `off` or `on`.
- **Amends**: [0563](0563-cache-keep-alive-speaks-one-grammar.md). The cache
  cards are a sub-setting of the keep-alive.

## Context

One Lucidos Agent wait in a release thread failed twice.

The agent armed an event wait. Half an hour later the user asked a question, a
real turn ran, and the agent went idle with the wait still armed. The status
kept the same run start, so the transcript kept updating the old card above the
user's message. The keep-alive looked stopped.

The budget then gambled and lost. The history cap gave 13 pings, about 56
minutes. The wait woke 71 seconds after the cache expired. The thread paid the
pings and a full reread, where no pings at all would have paid the reread only.

Three causes sat under it:

- One instant, the wait start, both named the run and dated the wait.
- The budget knew only history and prices. The agent knew the build was still
  running, and nothing let it say so.
- "Break-even" named the point where the pings have cost one reread. It read as
  if the pings had paid off.

## Decision

**A real turn mid-wait ends the run.** The *run start* is the later of the wait
start and the agent's last real idle. It names the run and its spend window. A
beat's own idle never counts. The give-up time still dates from the wait start.

**The expected wake drives the plan.** It is when the thread will next wake: the
earliest of its event waits and the children that report to it. The agent
states it (`expect_wake`, `lucidos expect-wake`). A turn that leaves anything
pending and states nothing gets one background decision call. It runs on a chat
model unless the user picks a System One model.

**History reaches the agent at the wait.** With the keep-alive on, an
`await_event` registration on a keep-warm event asks for the expected wake. It
shows how long past waits of the same agent and type lasted. The figures stay
out of the `expect_wake` tool description: a description that changed hourly
would break every turn's prompt cache.

**The reread cap prices every plan.** It is the pings that together cost one
reread. A plan looks forward from now, since past pings are sunk. It runs only
when its pings cost at most 80% of one reread, and it plans to 1.25 times the
expected wait. An estimate given as odds per time span picks the span end with
the largest expected saving. Ties keep the earlier end.

**A plan that runs out asks once.** It runs the expected wake decision again,
and that call sees the estimate that passed with no wake. Each estimate gets
one such call. A new estimate may ask again when its own plan runs out.

**Two caps hold whatever the estimates say.** No ping lands past an event wait's
own timeout, and no run spends more than one reread in total. The second is a
deliberate starting point, to revisit with data. A wake expected past the
timeout counts as the timeout, since the timeout wakes the thread anyway. An
overrun tail past the reread cap is clipped there. A wake itself past the cap
sends no pings, and the status reads `wake_past_reread_cap`.

**The mode is `off` or `on`.** Both old non-off values read as `on`, and the
default stays `off`. With no estimate at all, `on` falls back to the bare reread
cap from the wait start, half of it for a parked wait.

**The cache cards are a sub-setting of the keep-alive.** They draw only while the
mode is `on` and `cache_cards` is on. Their switch sits under the keep-alive's
own in Settings and shows only while the keep-alive is on.

## Rationale

The reread cap is the ski-rental rule. With no information, stopping at one
reread is never worse than about twice what hindsight would have paid. So it is
the right fallback and the right price for every plan.

An estimate beats history because the agent often knows what it waits for: a
build, a test suite, a sub-thread it just started. History says how long waits
of a type lasted elsewhere. It is an input to the estimate, shown to the agent
and the decision, never a rule.

The two margins keep a plan honest. The overrun margin covers a wake a little
late, and the saving margin refuses a plan whose saving is within noise. A plan
that fails the saving test sends no pings, and the status says the wake is
expected past the cap.

One re-estimate per estimate bounds the chain without a counter. The safety
cap bounds it again from the wait start.

The cache cards say what the keep-alive does. With the keep-alive off they have
nothing to report, so their switch had no meaning there.

## Consequences

- A user message never extends a wait by itself. A fresh estimate can.
- A card per run: a real turn closes the old card as `ended` and the next run
  draws below the newest exchange.
- Parked waits keep half the reread cap from the turn that asked. No model can
  estimate when a person answers.
- Codex threads stay unkept, and so do trigger threads no person has written in.
- The term "break-even" keeps only its true sense: a run is *expected to break
  even* when its expected wake falls inside the reread cap.
- Stored `break-even` and `workspace-data` values migrate to `on`.
- The card's "Show the math" table still shows the odds that history gives a
  warm wake. It counts them against the plan's pings, and sets no budget.

## Alternatives considered

- **A fresh budget per run.** A real turn mid-wait would grant a new reread cap
  each time. A chatty wait could then spend several rereads keeping one cache
  warm. Rejected: the safety cap is one reread per run, dated from the wait.
- **History as the rule.** The retired `workspace-data` mode picked the cheapest
  cap over 30 days of waits of the same type. It lost the release thread: the
  type's history said nothing about this build. Kept only as an input.
- **One estimate per pending item.** Each wait and child would carry its own
  wake. The thread wakes at the earliest of them, so one estimate per turn for
  the whole thread says the same with fewer moving parts.
- **Keep three modes.** `break-even` and `workspace-data` differed only in how
  they set a cap. With the plan priced by the reread cap, neither choice is left
  to the user.
