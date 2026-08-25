# 0103: The trimmer's six passes, and the honest verdict on persist-on-demand context

- **Status**: Accepted
- **Date**: 2026-08-22

## Context

ADR 0085 v3 makes an assembled body ride one round and leave, unless the model
calls `keep_in_context`. Eval run `07e4aa2ef0bc4317952150e4e363f433` is the
first result at the context ceiling. It ran both arms over 14 tasks on
`stealth/ox-alpha`.

The lean arm made **zero** `ContextKept` and **zero** `ContextDismissed` calls,
across 206 rounds and 14 threads. It cost 2.54x the control arm's input tokens
for identical completion results. Three graduation conditions failed, all
against lean.

The audit found the plumbing intact. `keep_in_context` is in every lean request,
which ships 68 tool schemas against control's 67. The `evt-<hex>` address round
trips through `event_address` and `parse_event_address`. Every body's event
exists in the arm database with a curatable type. The model can curate. It chose
not to, on every one of 206 rounds.

Two things then turned out to be broken underneath the mode.

**The blind trimmer destroyed the lean arm's context.** `trim_context_if_needed`
runs unconditionally in the agentic loop, under both arms. T14 lean spent 75
rounds and 4.65M input tokens against control's 6 rounds and 367k. Its round 1
issued five parallel `read_file` calls returning 193,527 chars, against a
167,668-char budget.

At the top of round 2 there were exactly three messages. `preserve_start`
collapsed onto `len`, because the tail was spared only above
`PRESERVE_RECENT_MESSAGES`. Pass 1 therefore swept `messages[1..3]`, and every
one of the five documents became a 40-char note before the model read a word.
Tool calls 6 through 76 are a re-read loop over the same five paths.

Control ran the same five reads. It split them 2+3 across two rounds, landing at
five messages, where pass 1's range is empty and the later passes needed more
than five. Control survived because the trimmer failed to fire. It shipped
220,900 chars over a 172,427-char budget and finished.

**The ledger contradicted the system prompt.** `ledger()` still carried version-2
opt-out prose. It said the model was holding everything listed, and named
`dismiss_from_context` alone. It never named `keep_in_context`. That text sat in
block 0, beside the addresses the model would have acted on.

## Decision

**1. The two evictors get disjoint territory, not a mode gate.** The mode owns
the assembled body region, which is block 1 of the pinned turn user message. The
trimmer owns tool-result history. No pass reaches the other side, and
`context.rs` stays free of `ContextMode`.

**2. The trimmer is six ordered passes, cheapest loss first.** Each stops the
moment the total is back under budget. Removal is last, because it is the only
pass that loses anything silently.

**3. Every stub states its size, and the addressed ones name the way back.** One
constant, `BUDGET_CUT_NOTE`, opens all of them. The bare `\n[evt-<hex>]` trailer
stays last, so `context_mode::stub_live_result` still finds a block the trimmer
already cut.

**4. The v3 persist-on-demand default is wrong, and the next run must prove it
or bury it.** The arithmetic is below. The flip is not made in this change,
because this run cannot separate a wrong default from a sabotaged arm.

## Rationale

### The pass table, and the decision on each

| Pass | What it cuts | Decision | Why |
|---|---|---|---|
| 0 | image bytes outside the last message and the pins | unchanged | Not a budget pass. It runs before the gate, and its pins are the fix for "the bot can't see my attached image". |
| 1 | oversized results and arguments in old messages | two fixes | The tail is now protected at every message count, and the walk stops at budget. |
| 2 | old assistant prose | new | Makes removal a backstop rather than the routine reclaim path. |
| 3 | outsized results in the preserved tail | stops at budget, stub now addressed | Its threshold was already right. Only its appetite was wrong. |
| 4 | the newest message's own results | new, keeps a head | Reached only when one message exceeds the whole budget alone. |
| 5 | oldest message pairs | unchanged mechanism, moved last | The only silent loss, so nothing above it may still be stubbable. |

**Pass 1's two fixes are the T14 root cause.** The old range was a function of
message count, not size. Below five messages it covered the whole array, so the
tail rule inverted exactly when the tail was all there was. The old walk also
cut everything in range whatever the shortfall: T14 needed 34 KB back and lost
193 KB. `preserve_start` is now `len.saturating_sub(PRESERVE_RECENT_MESSAGES)`
floored at 1, and the walk breaks as soon as the total fits.

**Pass 2 exists so that removal is rare.** Without it the ladder jumped straight
from stubbing tool results to deleting message pairs. Old assistant prose is
cheap to lose and the model can see a stub where a deletion leaves no trace. It
is role-gated to assistant messages, because a mid-turn user injection in the
same range carries a real instruction.

**Pass 4's gate is deliberately narrow.** Merely being over budget is usually
survivable. The budget counts a conservative 1.5 chars per token against a
measured 2.5, so it is roughly 40% tighter than the real window. Control proved
the point by shipping 28% over budget and finishing in six rounds. One message
over the *whole* budget is different: no eviction anywhere else can bring that
request under, so something in it has to go.

It head-truncates rather than erasing. A total stub guarantees the model
re-fetches what it just asked for, which is a livelock rather than a saving.
`LAST_RESORT_HEAD_CHARS` keeps 20,000 chars plus the address.

### The departure from the approved plan

The plan ordered the last-resort cut after pair removal, on the reasoning that
the newest message holds what the model is looking at. A test showed that order
removing four messages and then cutting the newest one anyway. While one message
exceeds the budget alone, pair removal provably cannot reach the budget. Every
pair it drops in that state is history lost silently and for nothing. The cut
now runs first, which strengthens the plan's no-silent-removal invariant rather
than weakening it.

### Why disjoint territory beats a mode gate

A `ContextMode` gate inside `context.rs` would create two trim paths that drift.
Worse, it would be wrong: every fix above is correct in both arms, and control
was saved by luck rather than by design. The T14 collapse was never
mode-specific. It was one message-count bug that the lean arm's batching walked
into first.

### The verdict on persist-on-demand

The cache arithmetic is the whole argument. A cached read costs 0.1x, a cache
write costs 1.25x, and an uncached token costs 1.0x.

Dropping a body of size B saves `0.1 * B` on each remaining round. Getting that
drop wrong costs a re-fetch: `1.25 * B` to write the body again, plus `0.1 * P`
where P is the prefix that the re-fetch invalidates. At a typical P of 100k and
B of 5k, the saving is 500 per round and the recovery is 16,250. One wrong drop
costs about 32 rounds of saving.

So the model has to be right about roughly 97% of its drops to break even. That
is not a bar a model clears by being careful. It is a bar that says the default
is on the wrong side.

The observed data agrees. Lean made 110 more recovery calls than control and
took 104 more rounds, at 2.54x the input tokens, for the same completion
results. Some of that is the trimmer, and this change removes that cause. None
of it is explained by curation, because there was none.

**The honest reading is that v3 asks for something a model will not do.** The
mode also charges rent whether or not it is used: 3,783 chars of system prompt
plus 976 chars of tool definitions, so 4,759 chars on every single request. Under
v2 opt-out, an unused mode degrades to control plus that bounded tax. Under v3
persist-on-demand, an unused mode degrades to a model re-fetching its own
context forever. The failure modes are not comparable, and only one of them is
survivable.

The next run has one job: hold everything else fixed and see whether a lean arm
with a working trimmer curates at all. If `ContextKept` is still zero, the
feature has no evidence left to stand on and should be retired rather than
tuned. If it is non-zero, the same run tells us whether the drops were accurate
enough to pay the 32-round recovery price. Either answer is worth more than
another round of prompt wording.

## Consequences

- Every trim loss is now legible to the model. It states the size, and where the
  content carried an address it names the `events` call that reads it back.
- Pair removal becomes rare. It is reached only after four stubbing passes have
  run out of material, which is what makes `messages_removed` meaningful again.
- The trimmer no longer punishes batched parallel tool calls. A round issuing
  five large reads keeps them.
- `guidance_hash` does not move, so the next eval run is comparable with this
  one. The fixture is untouched on purpose.
- The mode keeps its 4,759-char tax for at least one more run. That is the price
  of moving one variable at a time.
- ADR 0085's v3 default is now formally on probation. This ADR does not repeal
  it, and states what would.

## Alternatives considered

**Gate the trimmer on `ContextMode`.** Rejected. It assumes the trimmer is only
wrong under the mode, and the evidence says otherwise. It would also leave the
control arm running the same message-count bug, unfixed and unobserved.

**Disable the trimmer under the mode entirely.** Rejected. It drops pass 0's
image handling with everything else, and it leaves no answer for the round whose
own tool results exceed the budget. A request that cannot be made smaller is not
made smaller by having no code to make it smaller.

**Keep erasing oversized results instead of head-truncating.** Rejected. T14 is
the counter-example: 71 of its 76 tool calls were the model re-reading paths the
trimmer had erased. A stub with no content is an instruction to re-fetch.

**Flip the default back to v2 opt-out now.** Rejected for this change, not on
the merits. The arithmetic above says v2 is right. But flipping moves the
fixture and `guidance_hash`, which puts two variables in the next run and makes
its result unreadable. Fix the trimmer, re-run, then flip on evidence.

**Retire the mode now.** Rejected for the same reason. The one run that reached
the ceiling had its lean arm's context destroyed on round 2 of the hardest task.
That is not a fair test of the design, however poorly the design reads on paper.

## Amendment: the 97% bar was priced against the wrong invalidation

[ADR 0109](0109-model-writes-notes-and-sees-its-own-context.md) supersedes ADR
0085 and corrects the arithmetic above.

The 32-round recovery price assumed a wrong drop invalidates the whole prefix. A
tool result sits in the message array, so stubbing at position p invalidates only
the cached suffix after p. Per pass the cost is `1.15 * S`, where S is that
suffix, and the benefit is `0.1 * B` per later round. So a pass pays when
`B / S >= 11.5 / n`, with n the rounds remaining.

That rule is stricter than the old one in a different direction. Single-item
dismissal every round always loses, whatever the accuracy, and nothing pays late
in a thread. Only a large contiguous batch clears it, and a park boundary clears
it at zero cost because the suffix is rewritten anyway.

**Decision 4 is answered rather than repealed.** The next run did hold every
other variable, and the lean arm still made zero calls. 0109 finds the cause one
layer down: `ledger()` renders an address, a label and a recovery call, and no
size, age, total or percent of budget. The model was asked to curate without an
instrument.

**Decisions 1, 2 and 3 stand.** The two evictors keep disjoint territory and
`context.rs` stays free of `ContextMode`. The six passes keep their order and
their thresholds. Every stub still states its size and names the way back.

0109 adds two inputs to the trimmer and no pass. A pin set, so `keep_in_context`
means "never stub this at any budget". And an exemption for error results, so a
failed action stays visible to the model that has to avoid repeating it.
