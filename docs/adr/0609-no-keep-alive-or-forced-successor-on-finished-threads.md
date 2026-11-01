# 0609: No learned pings and no forced successor session on finished coding-agent threads: a real cold reply costs under 1% of spend once in-turn gaps are told apart

- **Status**: Accepted
- **Date**: 2026-10-31

## Context

A design thread asked what a cold reply to a finished coding-agent thread
should cost. Its brief covered 30 days on one heavy workspace, with about
$23,000 of spend. It counted 2,171 rewrites, about 18% of that spend, as "the
user replies 5 to 60 minutes after a finished turn". It weighed three options:

1. Learned pings on finished threads, capped per local hour as in ADR 0606.
2. Lock a finished session, so a cold reply starts a *successor session* from
   a *handoff brief*.
3. Both, chosen by context size and hour.

The brief's method took main-session requests over 20k input tokens whose cache
write exceeds half the input. It kept those more than 5 minutes after the
previous request. It named the cause by what lay between the two requests: a
card, an event wait, a session start, a child report, else idle.

[ADR 0470](0470-keep-the-prompt-cache-warm.md) rejected keep-alives on every
idle thread, since most idles never get a reply. ADR 0606's learned per-hour
cap seemed to answer that objection, which reopened the question.

## Decision

**Neither option ships.** A finished coding-agent thread gets no keep-alive
pings, and a cold reply resumes the same session as today. The *Successor card*
stays the only path to a fresh session on a big idle thread.

Revisit when a real cold reply passes about 2% of spend, measured with the
method under Consequences.

## Rationale

**"Else idle" also caught long tool calls inside a turn.** Such a gap holds no
card, wait, session start or child report either. Splitting the same 30 days by
whether a real idle and a user message lie between the two requests gives this.
Cost is the avoidable part, cache write minus cache read, at list prices:

| Cause | Rewrites | Avoidable cost | Avg context |
|---|---|---|---|
| Long tool call inside a turn | 2,065 | about $4,030 | 415k |
| Event wait | 353 | about $550 | 331k |
| Question card | 494 | about $545 | 236k |
| Real reply to a finished turn | 142 | about $200 (0.9%) | 296k |
| Child report | 87 | about $180 | 435k |

- **ADR 0470 already fixed the in-turn bucket.** It peaked at 793 rewrites in
  one week, then fell to 77 and 42 once waits returned within 230 s. Its p90
  gap was 10.5 minutes, the old `timeout: 600000`.
- **The real cold reply is small and shrinking.** In the 12 days after the fix,
  31 of them cost about $20 against about $3,300 of Opus spend (0.6%).
- **That figure caps every option.** Pings that miss, and ADR 0606's worst case
  of two rereads, would spend much of it. A successor's weaker start would spend
  more in re-reads and corrections.
- **Option 2 repeats the Successor card.** That card already offers a successor
  session to a big idle thread. It starts with a handoff brief on a cold cache,
  and the *handoff estimate* prices the cheaper calls that follow.
- **The break-even is not one number.** Opus 5.5 writes at 25 times a read, so
  25 pings cost one rewrite. Every other Claude model on the price card writes
  at 12.5 times, which halves it.

## Consequences

- Re-measure with this method. A rewrite counts as a cold reply only when a
  real idle (no `reason`) and a `MessageReceived` lie between the two
  main-session requests. A gap with neither is an in-turn tool call.
- Each reply to a coding-agent thread opens a resumed `SessionStarted`. So a
  "new session" bucket that counts every `SessionStarted` mostly holds replies.
- The in-turn rewrites left after the fix are mostly `harden-suites.sh wait`
  and `background-task wait` calls running past 5 minutes. That is ADR 0470's
  territory, not this one.
- The evidence is one heavy workspace at one price ratio. A workspace whose
  users reply more often after a gap could cross the revisit line sooner. The
  method lets any workspace check its own figure.

## Alternatives considered

- **Learned pings on finished threads (option 1).** The hour-learned cap answers
  ADR 0470's objection that most idles never get a reply. It still loses: the
  whole prize is under 1% of spend, before the pings are paid.
- **A forced successor session on a cold reply (option 2).** It saves the
  rewrite and shrinks every later call. The Successor card already offers that
  where the handoff estimate pays, and forcing it overrides the user's choice.
- **Both, by context size and hour (option 3).** It carries the costs of both
  for the same small prize.
