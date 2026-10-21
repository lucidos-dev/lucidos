# 0472: Trigger runs decide their own read, and the read decision knows what the turn already pushed

- **Status**: Accepted
- **Date**: 2026-10-14
- **Amends**: [0417](0417-turn-end-gate-enforces-valid-states.md), [0426](0426-read-decision-leads-with-gemini-3-8-flash.md), [0433](0433-turn-end-decisions-are-judgment-sites.md), [0435](0435-wake-decision-shares-the-read-decisions-default.md)
- **Amended by**: [0477](0477-read-decision-predicates-and-steadiness-measured.md), which measured the deferred split predicates and ways to steady the chat judge

Record: `docs/plans/2026-10-14-trigger-runs-out-of-review.md`.

## Context

Routine trigger runs landed in Review. The maintainer: "Can't have noise that
should not be read end up in Review." Two shapes made most of it:

- **A run that found nothing.** The `TRIGGERS:` rule told such a run to "just
  finish", which left its read decision to the forced call. That call said yes
  to a price check that reported "Nothing changed" and listed what it checked.
- **A run that already pushed.** An inbox run sent the news as a notification,
  then ended undecided. The forced call never saw the push, said yes, and the
  same news reached Review a second time.

On a packaged install, trigger agents left 24 of 44 runs to the forced call.

## Decision

- **Every trigger turn makes its own read decision.** The `TRIGGERS:` rule asks
  for `request_read` false when the run found nothing new. It is also false
  when a notification the run sent already carried the news (amended below:
  false after any notification). It is true only
  for findings the user would otherwise miss. `triggers.md` says the same.
  The rule costs 30 characters of always-loaded context
  (`ALWAYS_LOADED_BUDGET_CHARS` is now 131,198).
- **The forced call reads what the turn pushed.** A `send_notification` the
  turn delivered, after its opener and with a successful result, reaches the
  call as a closing note with its title and message.
- **Its question treats a push as a headline.** A reply that restates the push,
  says it was sent, repeats what an app now shows, or adds bookkeeping is a no.
  Details the user needs to act on still read (amended below: they no longer
  do). A run with nothing new is a no,
  even when it lists what it checked or repeats standing items. A report the
  trigger asks for on every run still reads.
- **The read decision defaults to its chat model**, led by
  `gemini-3.8-flash`, then `gpt-5.6-luna`. OpenAI Decisions stays pickable. The
  wake decision keeps OpenAI Decisions as its own default,
  `WAKE_DECISION_DEFAULT`. The maintainer chose this after the numbers below.
- `READ_THRESHOLD` stays 0.5, and a failed call or a missing answer still asks
  to be read.

## Rationale

`lucidos-eval read-decision` scores each backend through the gate's own prompt.
Two sets:

- **Public:** `eval/read-decision/turns.toml`, now 76 synthetic turns (29
  must-read, 47 noise), 16 of them new trigger-run shapes.
- **Private:** 78 real trigger runs sampled from three workspaces, labelled by
  hand: 17 must-read, 61 noise. It stays outside the tree. One label changed on
  review: a run whose push already named the target, issue, priority and reply
  angle is "pushed already", not a must-read.

"Before" is today's question on today's engine, which cannot see a push.
"After" is the final wording, with the push note.

**Real trigger runs**, missed must-read / noise passed:

| Backend | Before | After (runs) | p50 ms | Tokens in / out |
|---|---|---|---|---|
| OpenAI Decisions (`gpt-6-luna`) | 0 / 45 | 1 / 22 (1) | 255 | 1,340 / 0 |
| `gemini-3.8-flash`, chat | 0 / 21 | 0 to 2 / 2 to 3 (5) | 2,100 | 1,556 / 130 |
| `gemini-3-flash-preview`, chat | 0 / 32 | 1 to 2 / 3 to 5 (5) | 1,060 | 1,556 / 14 |
| `gpt-5.6-luna`, chat | 0 / 52 | 0 / 29 to 35 (3) | 1,070 | 1,482 / 19 |

**Public set**, missed must-read / noise passed:

| Backend | Before | After |
|---|---|---|
| OpenAI Decisions | 0 / 9 | 0 / 3 |
| `gemini-3.8-flash` | 0 / 8 | 0 / 1 |
| `gemini-3-flash-preview` | 0 / 9 | 0 / 2 |
| `gpt-5.6-luna` | 0 / 13 | 0 / 12 |

- **The push note and the wording do most of the work.** On the same model,
  noise on real runs fell from 21 to 2 (`gemini-3.8-flash`) and from 45 to 22
  (OpenAI Decisions).
- **OpenAI Decisions did not keep up.** Across five wordings it passed 15 to 23
  noise runs and missed 1 to 5 must-reads. It answers with no reasoning tokens,
  and its guide asks for predicates "around observable criteria", where this
  question is one long policy full of exceptions. It is deterministic, so the
  numbers are stable, not noisy.
- **`gemini-3.8-flash` is the quietest, but it varies.** It runs at `low`, since
  `none` is not offered, and it does not answer the same way twice. Its misses
  were a teaser-push report, a confirmation that also kicked off a session, and
  a nightly plan that repeats the same job. Each was right on another run.
- **The threshold is not the lever.** The chat models answer at 0 or 1 almost
  always. OpenAI Decisions trades misses for noise at every threshold from 0.3
  to 0.9.

Cost per forced decision, at October 2026 list prices: OpenAI Decisions about
$0.00013, `gemini-3.8-flash` about $0.0017 (double from 1 January 2027, when its
introductory price ends), `gpt-5.6-luna` about $0.0003.

## Consequences

- **Trigger turns no longer decide "only when sure"** (ADR 0417). On
  always-thinking Claude and on Gemini, text beside a tool call is not shown
  as the reply. So a call on its own round costs a second full model round.
  The trigger addendum asks for the call beside the run's last other tool
  call, which costs no extra round. Other turns keep ADR 0417's rule.
- Fewer trigger runs reach the forced call at all, since their agents decide.
  The agent side cannot be measured offline: the forced share of trigger runs
  is checked on live workspaces after the change ships.
- On `gemini-3.8-flash`, a forced read decision waits about 2 seconds at p50
  and up to 8 at p95, up from a quarter second. It costs about 12 times more,
  which at one workspace's volume is under a dollar a month.
- An OpenAI-only install now decides on `gpt-5.6-luna`: no missed must-read,
  but about half of the routine runs it judges still land in Review.
- A forced decision on `gemini-3.8-flash` can miss a must-read on roughly 1 in
  17 undecided trigger runs with a real finding. The agent's own decision comes
  first, so this applies only to runs whose agent skipped it.
- `WAKE_DECISION_DEFAULT` replaces `TURN_END_DECISION_DEFAULT`, and
  `WAKE_DECISION_DEFAULT_ENDPOINT` its TS copy.
- The real set is three workspaces, labelled by one agent. Rerun both sets
  before any later change.

## Amendment: a push replaces the read

A price check pushed "Model prices changed" and also asked to be read, so the
news reached the user twice. The maintainer: "Don't need both." The agent rule,
`triggers.md` and the judgment's question now say one thing. A turn that sent a
notification is a no, unless the reply holds a failure the notification does not
mention. Details beyond the push no longer earn a read. The eval's three
`trigger-pushed-more` turns are now `trigger-pushed` noes, and a
`trigger-pushed-failure` turn covers the exception.

It stays guidance, not code: the deterministic rule below is still rejected.
Record: `docs/plans/2026-10-19-a-notification-replaces-the-read-request.md`.

## Alternatives considered

- **Keep OpenAI Decisions as the default.** Lost: 22 of 61 routine runs still
  reach Review, with a missed must-read.
- **Split the read decision into crisp predicates for OpenAI Decisions**, such
  as "Did the run find something new?" and "Does the reply only restate the
  push?", combined in code. Likely what that API is built for, and cheap, but a
  new design for one backend. Measured and rejected in ADR 0477.
- **A deterministic rule: a push means no.** Lost: a push can be a teaser, and
  the reply can hold a failure. The amendment above keeps it out of code, but
  no longer treats a teaser push as a reason to read.
- **Nudge the trigger agent until it decides.** Lost: the forced call is the
  backstop by design (ADR 0417), and a nudge costs a model turn per run.
- **Raise the threshold.** Lost: see Rationale, the threshold is not the lever.
