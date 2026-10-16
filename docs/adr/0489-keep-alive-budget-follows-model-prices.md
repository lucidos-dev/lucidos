# 0489: The keep-alive gives up at break-even: beats cost one rewrite, priced from an engine model catalog

- **Status**: Accepted
- **Date**: 2026-10-16
- **Amends**: [0470](0470-keep-the-prompt-cache-warm.md) and
  [0479](0479-keep-alive-follows-the-observed-cache-ttl.md). Both gave up a
  fixed 150 minutes after the wait began, for every model and both TTLs.
- **Amended by**: [0492](0492-keep-alive-beat-lands-near-4-5-minutes.md). The
  5-minute beat window starts at 230 s, so its budget windows are 230 s each.

## Context

The 150 minutes came from one simulation over 5-minute Claude Opus 5.5
sessions. The economics depend on one ratio: what a cache rewrite costs against
what one beat costs. A beat is a full-context cache read. A wake after expiry is
a full-context write at the session's TTL.

That ratio differs by model and TTL. A 5-minute write over a read is 12.5 for a
standard-price model, 25 for Opus 5.5 and 50 for Fable 5.1. A 1-hour write is 20,
40 and 80. So 150 minutes is about right for Opus 5.5 at 5 minutes. It is too
long for a standard-price model. It is far too short for any 1-hour session,
where one beat costs a small fraction of the rewrite it saves.

No provider publishes prices in a machine-readable form. The Anthropic Models
API returns the context window, the output cap and capabilities only. Claude
Code reports one cost total per turn, which cannot split a read from a write.

## Decision

The keep-alive gives up when the beats spent would cost one cache rewrite. The
budget in beats is the session's write rate for its TTL, divided by its read
rate. The engine gives up that many beat windows after the wait began: 200 s each
for a 5-minute cache, 3300 s each for a 1-hour one.

The rates come from `llm::model_prices`, one engine catalog of list prices with
a source URL on each row. The session's model is the `model` of the capture
that decides its TTL, matched through `served_as_sent`. An unknown model prices
at the standard multipliers.

## Rationale

- **Break-even is the optimal deterministic rule.** This is the ski-rental
  problem: the wait length is unknown, and the rule never pays more than twice
  the cost in hindsight. The worst case is a wait that outlasts the budget. It
  pays the beats and then the rewrite, which is two rewrites at most.
- **The measurement agrees.** Over 1,425 keep-warm waits from 30 days, cost
  against never beating was nearly flat past the break-even budget and steeply
  worse below it. At ratio 25, 25 beats cost 22.3 percent and never stopping
  22.4.
- **A beat pays off while its chance of saving the rewrite exceeds read over
  write.** That is 4 percent at ratio 25. The measured chance stayed above that
  for as long as the data had samples.
- **The fixed figure could only be right for one model.** At standard prices it
  lets the double payment reach 4.4 rewrites. At a 1-hour TTL it stops after the
  second beat and a long wait pays a 2x rewrite that 0.05x beats would have saved.
- **One catalog prices every model once.** The compactor's cost estimate had
  its own table. The two now read the same rows, so a price change lands once.

## Consequences

- A 5-minute Opus 5.5 session beats for 5,000 s (25 beats). Opus 5 and standard
  models beat for 2,500 s. Fable 5.1 beats for 10,000 s.
- A 1-hour session beats for 20 to 80 windows of 3300 s, which is many hours.
  A stuck wait that long costs one beat an hour.
- An unknown Claude model gets the shortest budget, 12.5 beats. Wasted beats cost
  money every time, and too short a budget costs at most the one rewrite the
  keep-alive would not have saved anyway.
- A new Claude model needs a catalog row. Until it has one it prices as standard.
  A test pins that no Claude row has a lower ratio than that fallback.
- A beat's own small cost (its new prompt tokens, the one-word reply, any
  thinking) is not priced. It measures about 5 percent of the read.
- The beat windows and the keep-warm kinds do not change.

## Alternatives considered

- **Keep the fixed 150 minutes.** It is right for one model and one TTL, and it
  lets the worst case reach 4.4 rewrites on a standard-price model.
- **Fetch prices live.** No provider API serves them. A scraper of the pricing
  page is a new network dependency in the engine for the sake of one ratio.
- **Read Claude Code's reported cost.** It is one total per turn, so it cannot
  separate the read rate from the write rate.
- **Let the user edit prices.** Offered and declined. The built-in catalog
  covers the models in use, and the fallback is safe for the rest.
- **Price a beat's extra tokens.** It would shorten every budget by about one
  beat, for a 5 percent effect.
- **Stop at a fixed chance per beat.** It needs a model of the wait length,
  where break-even needs only the two prices.
