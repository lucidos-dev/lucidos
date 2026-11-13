# 0477: OpenAI Decisions keeps one policy question for the read decision; split predicates and chat steadiness measured

- **Status**: Accepted
- **Date**: 2026-10-14
- **Amends**: [0472](0472-trigger-runs-decide-their-read.md)

Record: `docs/plans/2026-10-14-read-decision-split-predicates.md`.

## Context

ADR 0472 moved the read decision's default from OpenAI Decisions to its chat
model, `gemini-3.8-flash`. It deferred one idea: ask OpenAI Decisions several
crisp predicates instead of one long policy, and combine the answers in code.
Its guide asks for predicates "around observable criteria".

The maintainer added a second question: the new default varies. Across runs it
misses 0 to 2 of 17 real must-reads, and each miss is right on another run. Can
asking twice, or a model that runs at `none`, steady it?

## Decision

- **OpenAI Decisions keeps the one policy question.** Split predicates lost on
  the number that decides first, a missed must-read, and never came near the
  chat default on noise. This change reverted the engine half.
- **The read decision's default stays `gemini-3.8-flash`.** The steadiness
  numbers below go to the maintainer. Neither candidate is built yet.
- `lucidos-eval` records each case's index, so repeat runs pair by case and
  "ask twice" or "majority of three" can be scored offline.

## Rationale

Same two sets as ADR 0472: the public `eval/read-decision/turns.toml` (76
turns, 29 must-read) and the private set of 78 real trigger runs (17
must-read, 61 noise). Cells read missed must-read / noise passed.

**Split predicates on OpenAI Decisions.** Code chose which predicates to ask
from what it already knew about the turn: who opened the turn, whether it pushed,
whether work was in flight, whether the parent acts on the reply. Five rounds
of wording and rule, the last re-scored offline:

| Round | Public | Real |
|---|---|---|
| Policy question (today, reproduced) | 0 / 3 | 1 / 22 |
| 1: seven first-draft predicates | 3 / 10 | 1 / 38 |
| 2: split failure and decision, positive "found new" | 1 / 14 | 3 / 19 |
| 3: done report, sharper "report every run" | 1 / 5 | 1 / 21 |
| 4: "act on the push alone", "says quiet", "new failure" | 3 / 4 | 6 / 6 |
| Best rule over round 4's answers, offline | 1 / 4 | 1 / 16 |
| 5: that rule, plus "did the user ask a question it answers?" | 0 / 12 | 1 / 16 |

- **A push is where it fails.** Whether a reply adds what the user needs beyond
  a notification is a comparison. As a predicate, one wording says "details
  left out" to nearly every push, and its mirror says "enough alone" to nearly
  every push. The first passed 11 of 21 noise pushes. The second missed 5 of 6
  must-reads.
- **"New" is the other gap.** A standing problem and a new one read alike to a
  predicate, so "is there a problem" fires on runs that found nothing.
- OpenAI billed the input once per request, not per predicate: about 2,000 tokens for
  13 predicates, against 1,350 for the policy question.

**Steadying the chat judge**, real set only, five runs each:

| Option | Missed (mean) | Noise (mean) | Per decision | p50 |
|---|---|---|---|---|
| `gemini-3.8-flash`, `low`, once (today) | 1 to 3 (1.6) | 2 to 3 (2.4) | $0.0017 | 1.9 s |
| `gemini-3.8-flash`, ask twice, read on either yes (10 pairs) | 0 to 2 (0.9) | 2 to 3 (2.9) | $0.0034 | 1.9 s in parallel |
| `gemini-3.8-flash`, majority of three | 1 to 2 (1.7) | 2 to 3 (2.7) | $0.0051 | 1.9 s in parallel |
| `gemini-3-flash-preview`, `none`, once | 0 to 1 (0.8) | 3 to 5 (4.0) | $0.0008 | 1.1 s |
| `gemini-3-flash-preview`, `none`, ask twice (10 pairs) | 0 to 1 (0.6) | 4 to 5 (4.7) | $0.0017 | 1.1 s in parallel |
| `gemini-3.5-flash`, `none` (2 runs) | 1 (1.0) | 6 to 7 | not priced | 7.6 s, p95 50 s |

- **The misses lean, they do not flip.** `gemini-3.8-flash` missed two cases
  most of the time. One is a confirmation that also kicks off a session (4
  runs of 5). The other is a nightly plan repeating last week's job (3 of 5).
  Asking twice rescues a case only when one call lands, so a bad pair remains.
- **`gemini-3-flash-preview` at `none` misses less and costs half,** but passes
  about 1.6 more noise runs, mostly the same pushed runs every time. It never
  missed the confirmation. It missed the nightly plan on 4 of 5.
- **No option clears both recurring misses.** Each is one case in 17, so the
  differences between the top three rows are about one case.
- We could not measure `claude-haiku-5-5`: Vertex does not serve it in this
  project's region.

## Consequences

- The openai row stays pickable for the read decision, at ADR 0472's numbers.
- A new chat model or an "ask twice" mode is the maintainer's call on the table
  above, with its own plan. "Ask twice" doubles the forced call's cost, and
  both answers must land before the deadline.
- `gemini-3-flash-preview` is a preview model and can be retired by its vendor.
- The sets are still three workspaces, labelled by one agent. Rerun both
  before any later change.

## Alternatives considered

- **Ship the predicates on the openai row anyway.** Real-set noise fell from
  22 to 16. But the public set lost a must-read, a plain answer to the user's
  question. We also fitted the best rule on the data that scored it. Lost
  on the first number.
- **Per-predicate thresholds.** Would have tuned the push trade-off to these 27
  pushed runs and nothing else. Not tried.
- **A predicate for each policy clause the model got wrong.** "Did the user ask
  a question the reply answers?" fixed the missed answer but fired on every
  confirmation, adding 8 noise turns. Each such patch traded one case for
  others.
