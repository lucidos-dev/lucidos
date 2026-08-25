# 0109: The model writes notes and sees its own context: the panel, the scratchpad and the pin

- **Status**: Accepted
- **Date**: 2026-08-23

Supersedes [ADR 0085](0085-model-curates-what-survives-round-one.md) and amends
[ADR 0103](0103-context-trim-passes-and-the-persist-on-demand-verdict.md).
Its record is
`docs/plans/2026-08-23-model-scratchpad-and-auto-dismissed-tool-results.md`.

## Context

The lean context mode made zero `keep_in_context` and zero `dismiss_from_context`
calls across six eval runs. ADR 0103 fixed the trimmer that had destroyed the
lean arm's context and said the next run had one job: see whether the model
curates at all. It did not. Run 5 declined on all 206 rounds. Run 6 declined on
all 28 threads.

ADR 0103 read that as the default being on the wrong side. This ADR finds a
simpler cause underneath it.

### The model was curating blind

`ledger()` renders one line per body: address, label, recovery call. That is all
it renders.

No size. No age. No total. No percent of budget. The engine computes every one of
those numbers, for the trimmer and for the capture event, and sends the model
none of them.

So a model with roughly 3.5x headroom was asked to decide what to drop. Nothing
told it how full it was, or how big anything was. Declining was the correct
answer, and it could not see when it stopped being correct. The trimmer fired on
6 of 89 control rounds and 4 of 93 lean.

### The measured reason

VISTA (arXiv 2606.30005) names this **proprioceptive blindness** and measures it.
Without a state panel, median relative error on a model's own context size ran
0.43 to 0.84 across four backbones. With one, 0.00. Block-size error fell from
0.24 to 0.37, down to 0.00 to 0.02.

Its ablation is our exact situation: tools present, panel absent. That arm scored
37.3 on LOCA-Bench and burned 5.25M tokens. With the panel: 50.7 and 2.86M. So
the missing panel is worse **and** dearer. Claude Sonnet 4.5 went 8.0 to 34.7.

The benefit appears only under load. At small inputs it is a wash. At 256k it is
32.0 against 12.0.

The conclusion that matters here is that **the bottleneck is a missing interface,
not a missing policy.**

### The second missing half

Three things exist and none of them is a place to write down what you learned.
`todo_write` tracks what to do, not what is true. Long-term memory is
cross-thread, extracted afterwards by an auxiliary model, and assembled by the
engine. `keep_in_context` and `dismiss_from_context` only evict.

So with a delete key and no write primitive, a 40,000-char result offers keep all
or lose all. Eviction takes those chars to zero. A note takes them to 500 and
keeps the meaning.

### What the mode already got right

Recoverability. Every result carries an `evt-<hex>` trailer and
`query_events(event_id=…)` reads it back.

VISTA's Proposition 1 bounds any non-recovering method, deletion, masking,
summarization or skeleton compression, at `Pr[correct] <= B/(Nk) + 1/k`. A
recovering method succeeds with probability 1 when the handles and one recovered
block fit. So the trailer stays on everything.

### The real numbers, from five months of production

Every figure in ADR 0085 and ADR 0103 came from the eval, which runs 14 short
tasks back to back. That regime has no parks, no idle and no long threads. The
dev workspace has all three: 41,710 main-LLM requests, 2026-03 to 2026-08, each
carrying the provider's own cache counters and a per-section char breakdown.

| Region | % of input bill | Model-evictable |
|---|---|---|
| Live message array (`Conversation` plus resumed tail) | **33.1** | yes |
| System instructions | 27.6 | no |
| Tool definitions | 18.2 | no |
| knowhow docs | 9.9 | yes |
| Conversation history | 4.5 | yes |
| Long-term memory | 1.9 | yes |
| File list, app context, user message | 2.9 | yes |
| User profile, OAuth, device, credentials, clock | 1.8 | no |

Model-evictable is 52.4%. Static is 47.6%. The mode curates **1.71%** of a
request today, and its three sections reach 16.3%.

`Conversation` is the bytes the tool loop added, measured as the delta between
the live total and the assembled sections. It is the live message array under
another name, and it is the single largest addressable block. It is also the
growth term: 3,373 chars at round 1, 92,770 at round 13 and beyond, a 27x range.
That is why the mode looks harmless in short threads and does nothing in long
ones, and why the eval never saw the problem.

## Decision

**The objective is focus, not bytes saved.** What the model can attend to next
round is the thing being bought. Fewer tokens is a side effect, welcome and
measured, and never the reason.

That reordering is what lets a per-round expiry ship at all. Priced purely in
bytes it looks marginal, because the saving is small against the cache write.
Priced in attention it is the only schedule that works: a result the model has
read is clutter on the very next round, and a park can be 75 rounds away.

Three things ship together, all behind the existing `context_mode_experimental`
preference. A workspace with the mode off is unchanged.

**1. A context panel rides at the tail of every turn.** Percent of budget used,
total tokens, and one row per addressable item carrying its address, its size,
its age in rounds and its status.

**2. A scratchpad the model writes itself.** Replace-whole-document,
thread-scoped, notes anchored to the address they replaced. Persisted as
`ScratchpadWritten`. The latest version is pinned and rides at the tail beside
the panel.

**3. A keep buys exactly one more round.** `keep_in_context` stops feeding the
mode's arrival window and starts exempting an item from the next round's stub.
It is asserted while the result is in front of the model, and it lapses the
round after. Holding something means saying so again, every round.

**A turn is not the unit, and the difference is the whole design.** A hard task
runs 75 rounds inside one turn, so a turn-scoped keep is a standing keep wearing
a deadline. That is ADR 0085's recorded error: a kept knowhow body was the one
thing the model could not un-choose. What carries across a turn is the
scratchpad, never a keep.

It buys exemption from STUBBING, never from removal. Pass 5 at the wall is what
makes a request fit at all. A keep that could freeze it would be a way to blow
the context window.

Around those three:

**4. Every round is the unit, and the boundaries only add a nudge.** The
disposition happens in the round the result arrives, because that is the round
the model is looking at it. Turn end and the two park boundaries add a reminder
to bring the scratchpad current, and nothing more. They are not where the work
happens.

**5. The engine never decides what leaves.** Everything leaves uniformly one
round after it arrives, so the engine picks no victims. The model picks what
stays. `trim_context_if_needed` remains the backstop at the wall, and no timer,
budget fraction or idle gap ever evicts anything.

**6. No summarization anywhere.** The model writes every note.

**7. The conversation summariser is off under this mode**, and untouched with the
mode off.

**8. Errors and failed actions are never dismissed automatically.**

**9. `dismiss_from_context` goes**, along with the assembled body region, the
context ledger and the arrival window. The panel replaces the ledger, and the
keep replaces both verbs. One pin remains and it is not the model's: the latest
scratchpad, which no pass may stub, because it is the compressed form of
everything already let go.

**10. The recovery handle stays on everything.**

## Rationale

### Nobody ships the note

Everyone ships eviction. The gap is specific.

| System | Who decides what leaves | What replaces the bytes | Per-item note |
|---|---|---|---|
| Anthropic `clear_tool_uses` | runtime threshold | placeholder | no |
| Claude Code microcompact | runtime timer | placeholder | no |
| Claude Code compact | runtime threshold | one conversation summary | no |
| TokenPilot | a separate estimator model | head+tail preview, hash store | no |
| Less Context (C4) | fixed keep-last-N rule | one generic batched summary | no |
| VISTA | the working model | archived, recoverable | no, archive only |
| MemGPT / Letta | the working model | memory blocks it wrote | not per-item, not forced |
| Manus | the agent | the URL or path | not per-item |
| **This ADR** | **the working model** | **a note it wrote, per item** | **yes** |

Each of them either has a runtime decide, or lets the model choose only between
keep and archive, or writes one bulk summary.

Three groups each removed a different one of our three pieces and measured the
loss. That is why all three ship together rather than in sequence.

| Paper | Component removed | Result |
|---|---|---|
| Less Context, C3 against C4 | the note | 79.0% to 91.6% when added; premature termination 18 runs to 3 |
| TokenPilot | the recovery handle | cost rose $2.79 to $4.03, score fell 80.9 to 77.1 |
| VISTA | the state panel | score 50.7 to 37.3, tokens 2.86M to 5.25M |

Dismissing without a scratchpad is T14 again: 71 of 76 tool calls re-reading
paths the trimmer had erased. A notice without a note is run 5 again, an
unanswerable choice. A panel without either is state nobody can act on.

### The corrected arithmetic, and why the earlier table was wrong

### The bill, and why it stopped being a veto

ADR 0103 priced one wrong drop at about 32 rounds of the saving it bought. It
concluded the model had to be right about 97% of its drops. That bar assumed a
whole-prefix invalidation on recovery.

A tool result sits in the message array, so stubbing at position p invalidates
the cached suffix after p rather than the whole prompt. Per round the cost is
`1.15 * S`, where S is the invalidated suffix. The saving is `0.1 * B` per later
round, where B is the bytes removed. Break-even on cost alone is:

> **B / S >= 11.5 / n**, with n the rounds remaining.

| Situation | B/S needed for the bytes to pay for themselves |
|---|---|
| 40 rounds left | 0.29 |
| 20 rounds left | 0.58 |
| 5 rounds left | 2.3 |

**This table is a bill, not a gate.** It says what uniform expiry costs, and the
answer is roughly `1.15 * S` per round. It is a number to report on a run, next
to the accuracy the expiry bought. It becomes a veto again only if that accuracy
gain measures zero, and then the whole design fails with it.

Read as a gate it argues the opposite of decision 5, and it is worth being exact
about why it does not apply. The old reading assumed the only payoff is bytes not
re-read. Under uniform expiry the payoff is the model's attention next round, and
the byte saving is a side effect. So `B / S` prices the mechanism; it does not
decide whether to have one.

Two other results land on the byte reading, and inherit the same demotion.
Anthropic advises `clear_at_least` above 5,000 tokens. TokenPilot dismisses in
batches of three turns and reports that a batch of one inflates cache misses.
Both are optimising a bill. Neither was measuring what the model could attend to.

Cutting at the tail also makes the bill small. Uniform expiry always stubs the
round before last, two messages from the end, and everything after it is new and
unwritten anyway. For a 40,000-char result the stub costs about 190 units where
the cached read would have cost 4,000.

The rule still reframes the 97% bar. With a live handle a wrong expiry costs a
re-fetch, not 32 rounds.

### Amendment: the bill was right, and the wire did not let us pay it

The section above prices a stub at `1.15 * S`, with S the invalidated suffix. It
argues that cutting at the tail keeps S small. Measured in production, S was the
whole message array on every round, and the mode cost 5.6x per token of context
carried.

The reason is not the cut. A cache is readable only at a breakpoint, and
`anthropic_wire.rs` spent its only message-tier breakpoint on the last message.
That is precisely the message the next round rewrites. It holds the panel this
round appended, and the results this round read. With nothing behind the edit
but the system block, the arithmetic above had no place to read from.

The fix is a fourth breakpoint on the message in front of the tail, which the
next round leaves alone. With it, S is the tail after the anchor, and the
section above describes what the mode actually costs.

Two things follow for this ADR. Uniform per-round expiry is affordable, so the
bulk-expiry alternative stays rejected. And the scratchpad cap becomes the term
that decides whether the mode pays. At the shipped 8,000 chars the pad needs
47,150 shed tokens to break even, where 2,000 chars needs 19,550.

`docs/investigations/2026-08-23-context-mode-cache-breakpoint-diagnosis.md`
carries the measurements. They include the arms separating this from a second
defect: a round of 9 or more parallel tool calls breaks caching in **either**
mode, because consecutive tail markers then sit more than 20 blocks apart.

### Two clocks, and the bug of giving them one name

The old text used "when should this leave" for two different questions, and that
is what let a billing rule veto an attention decision.

| | Read clock | Pay clock |
|---|---|---|
| Question | What did this result tell me? | When is the cache write billed? |
| Unit | the **round** | the **park** |
| Why | it is the only round the body is in front of the model | a resume owes the write anyway |

The read clock is the design. A result gets one round of attention. So the
disposition happens in that round, and the bytes are gone in the next one.
Waiting for a park leaves the clutter in front of the model for the whole turn.
On a 75-round turn that is exactly where focus matters most.

The pay clock is a billing optimisation on top. It changes nothing about what
the model sees.

### Why there is no engine-timed dismissal, and why uniform expiry is not one

Claude Code clears after an idle gap, because the cache TTL has expired anyway.
That is not available to us. After the gap the model is not running, and
anything smarter needs a model call. Such a call loads the context back in and
pays the write we were trying to avoid.

**The T14 objection is about SELECTION, not about timing.** What produced T14 was
an engine choosing WHICH items to erase, on a rule that had nothing to do with
the task. Uniform expiry chooses nothing. Every item leaves on the same schedule,
one round after it arrives, and the model decides what to carry forward. There
are no victims to pick wrongly.

So the ban stands exactly where it was written: no timer, budget fraction or
round count may single anything out. A schedule that applies to everything
equally is not that rule, and reading it as one costs the whole design.

Our own cache cliff is at five minutes, so we run on the standard TTL:

| Gap since previous request | requests | share of tokens written |
|---|---|---|
| under 1 min | 35,044 | 2.2% |
| 1 to 5 min | 3,571 | 13.2% |
| 5 to 60 min | 1,343 | 89.8% |
| 1 to 24 h | 348 | 92.0% |
| over 24 h | 9 | 92.5% |

The useful moment is earlier, at the park.

### The park boundary is where the bill is cheapest, and nothing more

Everything above is the pay clock. It decides where the write is cheapest, and
it does not decide what leaves, because the read clock already did that.

A Lucidos thread parks constantly, and a park can last hours, so the cache
expires. On resume the engine pays a full 1.25x write on whatever it left behind.
Compressing first is therefore free.

| | Expiry mid-turn | Compressing before a park |
|---|---|---|
| Cost of the edit | `1.15 * S`, small at the tail | 0. The suffix is rewritten anyway |
| Byte saving | `0.1 * B` per round | `1.25 * B` once, plus `0.1 * B` per round |
| What the model sees | clutter gone next round | clutter present all turn |

The last row is why a park cannot replace the round. It is a discount on a bill
the design was going to pay anyway, so the boundaries get a nudge and no
mechanism.

Which pauses actually evict, measured:

| Pause cause | occurrences | evicted | rate | avg resume write |
|---|---|---|---|---|
| User question | 1,505 | 395 | **26.2%** | 106,961 tok |
| Turn end or plain wait | 38,808 | 1,305 | 3.4% | 72,560 tok |
| Permission prompt | 2 | 0 | n/a | n/a |

A user question is 7.7x likelier to evict than an ordinary gap, and its resume
write is 47% larger. Turn ends evict far more often in absolute terms, purely on
volume. Permission prompts barely register on chat threads today, and the
boundary is nudged anyway because it costs one string.

Cold resumes are 4.1% of requests, 39.7% of all cache writes and 19.7% of the
input bill, and 43.3% of that write is evictable. So 8.5% of the total bill is
bytes we could have compressed before the park, redeemable at 1.25x instead of
0.1x.

### The summariser was already on the control side, and the mode retires it

`ChatProcessor` runs a conversation summariser past `HISTORY_COMPRESS_THRESHOLD`
of 15 messages, refreshing whenever `HISTORY_SUMMARY_REFRESH_AFTER` of 5 older
turns go uncovered. It calls an auxiliary model under
`ContextPurpose::ConversationSummary` and compresses the assistant side of the
older region (ADR 0102). It runs in **both arms**, and no earlier record counted
it.

Measured in run 6 it is small: two calls per arm, 4,882 input tokens control
against 5,258 lean, so 0.08% of each arm. Memory extraction is far larger, at 35
calls and about 61,000 input tokens per arm.

It is small for the same reason everything else was: thread length. The longest
thread ran 14 rounds, so the older region barely existed. The summariser bites in
long threads, which is the regime the eval never reached.

It matters for the design regardless. It is exactly the generic summarizer this
design rejects, already shipping, reading a growing region with a model that does
not know the task. Its own module comment records a real call at 94,903 chars.
Under this mode it is also redundant: the model writes notes as it goes, so the
party that knew what mattered has already compressed the older region.

So the credit for this mode is not only cache savings. It deletes an auxiliary
model call that compresses blind. It also replaces a per-5-turn refresh over an
unbounded region. Inline notes cost a few hundred chars each, written once at
output price.

### Why the model writes the note

An auxiliary summarizer does not know what the thread is doing. Handed a large
file it writes a generic precis and drops the one constant the task needed.

The model reading that file knows it wants the retry limit. It writes one line,
and the other 40,000 chars become disposable. That ratio comes from the task, not
from the text, so no summarizer can reach it.

### Why we build rather than turn on Anthropic's flag

`clear_tool_uses_20250919` clears the oldest tool results past a trigger, keeps
recent pairs, leaves the `tool_use` block visible and swaps each result for a
placeholder. `clear_at_least` exists because clearing does invalidate the cache.
It is a good mechanism and we may use it later for Anthropic models. It does not
solve this problem now, for four reasons.

- It is one provider, and we keep a model registry and rent the model.
- It clears server-side and invisibly, so our token accounting and our eval
  capture cannot see it.
- It carries no state panel, and the panel is the part the evidence says
  dominates.
- Its freed bytes go to a placeholder, which is the design that already measured
  zero here.

Two pieces of this design are confirmed by that survey rather than invented here.
The pre-clear warning is real: with the memory tool on, Claude is told before a
clear so it can save what matters. And restorable compression is Manus's core
trick, which is our address trailer.

### One lever already pulled

TokenPilot's single largest win is prefix stabilization, more than half its gain.
Ours is already stable: across 24,827 warm requests the tool definitions changed
mid-thread once, and the system prompt changed 45 times, so 0.18%. A mid-thread
system change costs 75,129 tokens of write against 3,479 when stable, a 21.6x
penalty. At 0.18% it is not worth a phase.

### Why the tail

Manus's rule is that changing anything earlier in the context destroys the cache.
A panel that changes every turn is the worst thing to put at the head. The tail is
also where attention is strongest, which is why Manus recites `todo.md` there.

The scratchpad rides at the tail for the same reason, written as a tool result so
each write lands at the end and invalidates nothing. Superseded versions are
ordinary stale results.

Replace-whole-document beats append so a wrong note gets corrected rather than
contradicted, which is Letta's canonical failure. `memory_replace` is the same
answer to the same problem.

## Consequences

- **The mode's fixed rent ROSE by 635 chars, and the plan expected it to fall.**
  `scratchpad` is a larger schema than the `dismiss_from_context` it replaced,
  at +575. The rule block gained 60, stating a deadline instead of a release
  rule. `ALWAYS_LOADED_BUDGET_CHARS` records the split.

  What fell is not on that meter. The context ledger and the body region rode in
  the user message of every lean request, sized with the bodies they described.
  Both are gone, and so is the summariser call. The panel replaces them and
  scales with the live item count. So the fixed half is dearer, the variable
  half is much cheaper, and only a run measures the net.
- **`keep_in_context` changes meaning.** It was "hold this assembled body for
  the life of the thread". It is now "do not stub this for one more round".
  Both events stay readable: existing workspaces hold `ContextKept` and
  `ContextDismissed` rows.
- **No keep outlives its round.** ADR 0085's amendment named the standing keep as
  a design error: a kept knowhow body was the one thing the model chose and could
  not un-choose. A one-round keep answers that without a second verb.
- **The do-nothing outcome is a re-fetch, not amnesia.** A result the model
  neither noted nor kept becomes a stub carrying its address, so the fact is one
  call away. That is the failure ADR 0103 called survivable, and it is bounded:
  the eval counts re-fetches, so a model that curates badly shows up as rounds
  spent rather than answers lost.
- **The trimmer gains one input and no new pass.** A protected-key set joins
  `protected_idx` and `keep_image_idxs`, carrying the round's keeps, every
  failed action, and the live scratchpad. The six passes keep their order and
  their thresholds.

  The scratchpad needs no key, and the reason is worth writing down. Its text
  is a `tool_use` argument, which pass 1 truncates to a 200-char preview over
  500 chars, against a pad cap of 8,000. So the pad is rendered at the tail of
  the newest message every round instead. Pass 1 does not reach there, pass 2
  skips it as non-assistant, and pass 5 cannot remove it. The duplicate in the
  arguments stays cuttable, which is what lets the budget reclaim it.

- **A protected thread can finish the trim over budget, and that is accepted.**
  Passes 1, 3 and 4 skip a protected key, and pass 5 cannot reach the preserved
  tail. ADR 0103 already calls shipping over budget correct when nothing is
  safe to cut. The budget also counts a conservative 1.5 chars per token
  against a measured 2.5.

  The one-round keep is what bounds it. A keep lapses unless the model
  re-asserts it, so the set holds one round's results. ADR 0085's standing keep
  would have accumulated a whole turn's. A failed action is an error message,
  which is small. An override that forced the total
  under the budget was written and then reverted: it cut the round the model was
  reading, which is the T14 livelock ADR 0103 exists to prevent.
- **`context.rs` stays synchronous and never calls a model.** Every request goes
  through it and its tests are pure.
- **A quoted excerpt goes stale and the address does not save it.** An event is
  immutable, so a re-fetch returns what the result said at the time, not what
  the file says now. Replace-whole-document is the mechanism that fixes a wrong
  entry, and the tool's own contract states the rule: change the thing an entry
  quotes, rewrite the entry in the same call.
- **A long excerpt in the pad is a keep worn as a note.** It moves bytes from a
  cheap stub, recoverable by address, into a document re-emitted at output price
  and re-read every round. Short fragments earn their place where re-deriving
  costs a round. Long ones are what `keep_in_context` is for.
- **A poisoned note outlives the result that carried it.** A note is pinned and
  persisted, so injected text in a tool result can survive. Notes are therefore
  rendered as model prose attributed as such, never as instructions.
- **Silent use-after-free is the sharpest risk.** The model keeps reasoning from
  evidence it can no longer inspect, and nothing signals it. The panel makes
  stubbed state visible rather than invisible, which is the whole mitigation.
- **The note has two quality bars, and they are different questions.** As
  compression you ask whether it preserved the fact. As a focusing device you
  ask whether it names the right thing to look at next round. A note that
  preserves every fact and states no problem passes the first bar and fails the
  second. The eval has to score the second one, or it measures a summariser.
- **Housekeeping can replace reasoning.** Letta's first documented failure is
  that memory edits feel productive and real output thins. Silence stays a valid
  answer and the nudge puts the user-facing work first.

  **The counter-claim is why a per-round disposition is worth trying at all:
  writing down what a result told you IS the work.** Naming the one fact that
  mattered out of 40,000 chars is what focusing on a problem looks like. Manus's
  one-third-of-actions figure came from rewriting a plan, which is a different
  act. The eval can settle this, by scoring answer quality against the
  disposition count rather than assuming a trade.
- **Blind offloading is the other direction of the same failure.** VISTA saw
  runs archive bulky evidence purely to free space and never read it back. The
  panel is what its ablation shows fixes this.
- **Accuracy can simply go down.** TokenPilot trails its do-nothing baseline on
  one of two benchmarks, 63.1 against 64.5 and 60.8 against 63.4. Less Context's
  prune-only arm caused premature termination in 18 runs against 3. No cost win
  is accepted without the retention task below.
- **The magnitudes are Anthropic's.** The 1.25x and 0.1x figures are theirs. The
  B/S rule holds for any prefix cache; the numbers in it do not.

### What the next eval run needs

Not built here. Recorded so the next run does not have to re-derive it.
[ADR 0110](0110-context-handling-benchmark.md) builds it, and the table after
the list says where each item landed.

1. **A budget-pressure knob, not bigger tasks.** What matters is the ratio of
   working set to budget, and every source says the benefit appears only under
   load. A multiplier puts the existing 14 tasks at the ceiling cheaply.
2. **Long threads under sustained pressure.** The longest thread today is 14
   rounds and 94.5% of rounds never trimmed.
3. **`capture_body` on**, so every run becomes a replayable dataset.
4. **A retention task**, which fails when an early round's content is lost.
   Otherwise the eval measures cost and calls it quality.
5. **A blindness probe.** Ask the model its own context size and score the error
   against the truth. That is the one number telling us the panel works.
6. **Count re-fetches**, and count the archive-then-recover loop.
7. **Count dispositions**: kept, noted, ignored.
8. **Charge both arms for auxiliary calls.** Sum `ContextCaptured` by purpose, so
   the summariser and the extractor land in the headline rather than beside it.

| Item | Where it landed in ADR 0110 |
|---|---|
| 1, a budget-pressure knob | the budget sweep, seeded as `models.context_window` |
| 2, long threads under pressure | the sweep, which applies pressure to all fourteen tasks |
| 3, `capture_body` on | superseded by the verbatim capture behind `eval-capture` |
| 4, a retention task | the fidelity scorer, which was already there and is now an axis |
| 5, a blindness probe | deferred, and named as the one axis 0110 does not build |
| 6, count re-fetches | the rounds axis |
| 7, count dispositions | dropped, as mechanism the criteria may not name |
| 8, charge auxiliary calls | the cost axis, summed by `ContextCaptured.purpose` |

Item 7 is the one 0110 refuses. Counting kept, noted and ignored is counting the
mechanism, and a benchmark of context handling has to survive the mechanism
changing. The capture holds every disposition verbatim, so the count is one
query away for anyone who wants it, and it reaches no score.

## Alternatives considered

**Keep persist-on-demand and tune the prompt.** Rejected. Six runs produced zero
calls. ADR 0103 already said one more round of prompt wording is worth less than
either answer from a working experiment. The panel is what makes the question
answerable at all.

**Summarize tool results.** Rejected, and recorded here so it is not
re-proposed. An auxiliary summarizer does not know the task, so it writes a
generic precis and drops the one constant that mattered. The whole compression
ratio comes from the task, not from the text.

**A SELECTIVE engine eviction rule.** Rejected in three forms: pick the oldest,
pick above a budget fraction, pick after an idle gap. Each has the engine
choosing WHICH items go, on a rule that knows nothing about the task, and that
is what produced T14. Uniform expiry is not in this family. It picks nothing:
everything leaves one round after it arrives, and the model chooses what to
carry forward.

**Expiry at the park boundaries instead of every round.** Rejected, and it is
the closest call here. It is cheaper, because the resume owes the write anyway.
It also fails the objective: a park can be 75 rounds away, so the clutter stays
in front of the model for the whole turn.

The boundaries keep a nudge, because compressing there is free. They carry no
mechanism.

**Adopt Anthropic's server-side context editing.** Deferred rather than
rejected, for the four reasons above. It stays a live option for Anthropic
models once the panel exists.

**Ship the panel first and the scratchpad later.** Rejected. Three separate
ablations each removed one of the three pieces and measured a real loss. A
partial ship therefore measures the ablation rather than the design.

**Flip the mode on by default.** Rejected. Nothing here is evidence yet, and the
plan's own risk list names two published cases where context management cost
accuracy. The preference stays, off by default, and the eval decides.

**Change memory extraction too.** Out of scope. It is the larger auxiliary
spend, at about 61,000 input tokens per arm. But it is cross-thread rather than
in-context, so it is a different problem.

**A user-facing panel.** Out of scope. The model-facing panel is not a UI. The
Context Viewer already renders the same figures from `ContextCaptured`.

## Amendment: the note is the bill, and the rule above does not price it

An audit of the shipped code against 30 days of production re-derived the
arithmetic from the price table rather than from the earlier records. Two things
came out. The eviction is cheaper than this page says. The note is far dearer,
and this page does not price it at all.

Nothing is repealed. Decisions 1 to 10 stand, the panel and the scratchpad stay,
and the mode stays off by default. What changes is the bill, the break-even, and
what the next run has to show.

### The measured baseline

The dev workspace, 30 days, `producer = main_llm`: 26,303 requests across 1,787
turns. Priced at the `claude-opus-5@default` row, so the ratios compare with
every earlier record. The workspace runs the `[1m]` tier, which is 1.5x on all
four prices and therefore scales every figure uniformly.

| Line | Spend | Share |
|---|---|---|
| cached reads | $4,521 | 45.1% |
| cache creation | $3,597 | 35.9% |
| output | $1,706 | 17.0% |
| uncached input | $194 | 1.9% |

**Two things in that paragraph are wrong, and the third amendment below carries
the correction.** The `claude-opus-5@default` row was 3x too high, so the four
figures are
$1,507, $1,199, $569 and $65. And there is no 1.5x long-context tier: Anthropic
prices the full 1M window at the standard rate for Claude 4.6 and later. Every
share, ratio and multiplier on this page is unaffected.

`Conversation` averages 88,134 chars a request and is the largest single block,
which confirms the region choice above. What it adds per round falls as a turn
lengthens: 6,540 chars in turns of 1 to 9 rounds, 4,113 in turns of 20 to 39,
and 2,340 in turns of 80 and over.

Rounds per turn run to a median of 7 and a 90th percentile of 36. Cost is paid
per round, so the round-weighted view is the one a cost verdict reads. It is far
longer, and the break-even table below carries it.

### The eviction is profitable on the round it happens

The rule above says a pass costs `1.15 * S`. Take S as the cached bytes AFTER
the evicted block, which is what the text says, and two terms are missing. The
full cost of replacing a body B with a stub s, with C cached after it, is:

> **`1.25*s + 1.15*C - 0.1*B` now, then `0.1*(B - s)` saved every later round.**

`1.25*s - 0.1*B` is NEGATIVE whenever the stub is under 8% of the body, and real
stubs run near 1%. So the eviction pays on the round it happens and keeps
paying. The worked example on this page already says this, at 190 units against
4,000. The formal rule and the `B/S >= 11.5/n` table under it do not, and they
are what a reader carries away. Read the example, not the table.

C is small by construction, because the stub always lands one round behind the
tail. It is the previous round's panel and scratchpad, and both were going to be
rewritten anyway. The exception is a lapsing keep: a result held for many rounds
and then let go is stubbed deep in the array, and C is then everything after it.
Nothing prices or warns about that.

### What the note costs, which is the term that decides it

An output token costs 5 input tokens and 50 cached-read tokens on Opus. A pad of
P tokens is re-emitted whole on every write, so each write costs `5*P`. Rendering
it at the tail costs `1.25*P` more per round. Averaged over a turn of n rounds,
against the control arm:

> **`1.25*(s + m + P + V) + 0.05*n*(s + m - r) + 5*w*P`**

Here s is the stub and m the collapse markers. V is the panel and r is the tool
bytes a round adds. And w is the share of rounds carrying a write. Break-even
against the measured r of 1,275 tokens a round, a 1,200-char panel, and the
round-weighted distribution above:

| What the pad carries | Break-even turn | Share of rounds above it |
|---|---|---|
| no pad, eviction and panel only | 13 | 78.0% |
| 1,000 chars, one round in three | 34 | 48.9% |
| 1,000 chars, every round | 57 | 31.7% |
| 3,000 chars, one round in three | 74 | 21.2% |
| 3,000 chars, every round | 144 | 5.6% |
| 8,000 chars, every round | 362 | 1.6% |

**The pad cap is the whole verdict.** At `MAX_SCRATCHPAD_CHARS` of 8,000, written
as often as the guidance asks, the mode is under water on 98% of production
rounds. At 1,000 chars written now and then it is above water on about half.
The tool invites the first: the cap is 8,000, and the guidance says a short note
is almost always worth writing.

### The trim path is not the argument, on this window

The trimmer fired on 68 of 26,303 rounds, 0.26%. Every one of the 68 was on the
131,072-token window, where it fires on 33.8% of rounds. On the 1M window it
fired **zero** times in 25,626 rounds. The largest request seen there was
746,377 estimated tokens against a 992,000-token budget.

So "the mode replaces the trim path" is dead on the window this workspace
actually runs. What is not dead is the reframe above: focus rather than bytes. A
result the model has read is clutter on the next round whether or not the
context is full.

One stated credit is worth nothing here. The conversation summariser the mode
retires has produced no auxiliary capture in 90 days.

### The verdict

**Keep the mechanism, cap the pad, and do not graduate on cost.** The region
choice is right and the eviction arithmetic is sound. The note is the term that
sinks it, and it is also the term the design says it cannot ship without.

Four things follow, and none of them is in this change.

- **`MAX_SCRATCHPAD_CHARS` of 8,000 is too high by roughly a factor of four.**
  Every write of a full pad costs 16,000 input-equivalent tokens. A cap near
  2,000 keeps the tool useful and keeps the bill inside what the eviction buys.
- **`todo_write`'s `notes` field should go under the mode.** Two
  replace-whole-document surfaces both billed at output price is the cost bug
  twice over. The wording contradiction between them is fixed in this change and
  the duplication is not.
- **The next run must report `5*w*P`.** Sum the pad bytes written per turn and
  print them beside the byte saving. A run that reports only input tokens will
  read as a win while the output line pays for it.
- **A keep is durable where it should not be.** Decision 3 makes a keep last one
  round, and `ContextKept` is meant only as the eval's disposition record. But
  `store::messages::fold_curation` still reads it as a standing verdict that
  cancels a `ContextDismissed`. Only a workspace that ran the old mode holds
  such a row, so nothing is broken today. Fold the two verbs separately before
  anything else starts reading `ContextKept`.

### What would overturn this

A run showing the panel measurably improves accuracy under load. The blindness
probe and the retention task in the list above are the instruments, and neither
has been run. VISTA measured 37.3 against 50.7 on that question and its ablation
is our exact situation. If the panel buys anything close to that here, the bill
stops deciding and this amendment becomes a note about sizing the pad.

The opposite result also overturns it, in the other direction. Suppose a run
where the model writes a full pad every round and gains nothing measurable. That
kills the mode outright, because the byte case cannot carry it alone at that pad
size.

## Amendment, 2026-08-24: the one-round rule is an interruption, and the pad's round was never priced

A design session read the eval arm databases and the last thirty days of
production. Three claims on this page do not survive it. The mode's shape
changes, its objective does not, and the record is
`docs/plans/2026-08-24-the-working-understanding-and-the-ten-round-window.md`,
which carries the measurements, the proposed guidance text and the parse rule.

### What this page got wrong

**The one-round rule is an interruption, not a schedule.** This page argued that
a result the model has read is clutter on the next round. The premise holds and
is now measured: four in five tool results are never used again after the round
they arrive. What the argument missed is the fifth that survives.

It is wanted a median of three rounds later, and 78% of read-backs land at
distance 2, which is the earliest this rule allows. So the rule forces a
re-fetch at the first possible moment, on the results that mattered.

**The panel was necessary and not sufficient.** This page rejected ADR 0103's
reading, that the default sat on the wrong side, in favour of a simpler cause
underneath: the model was curating blind. The panel shipped and the blindness is
cured. The note appeared, 37 times, and the panel earns that credit. **The keep
stayed at exactly zero, in 301 rounds across two models.** For the keep
specifically, ADR 0103 was right.

**The cost model priced the pad's bytes and never priced its round.** The
amendment above concluded the pad was the term deciding whether the mode pays.
It is, but not for the reason given. A lone `scratchpad` call consumes a whole
round in which no task progress happens, and Sol paid that on 82% of its notes.
That round is worth roughly nineteen rounds of simply holding the result the
note was replacing.

So the model was not failing to curate. It was correctly refusing to spend a
round on bookkeeping, by this page's own arithmetic.

### What replaces them

Nine decisions, all in the record. In short:

1. **A tool result lives ten rounds**, then the call and the result both leave.
   Nothing stands in their place.
2. **Recovery runs through the event log**, and the command is stated once in
   the standing instructions rather than five times per request.
3. **One verb, `keep_open`**, which sets an item's clock back to ten. **Silence
   keeps.** The verb becomes an observation at round ten, not a forecast at
   arrival.
4. **One document, the working understanding**, written as ordinary text in the
   same reply as the next tool call, so it costs no round. Append or rewrite
   whole. A constraints heading always renders.
5. **The name is part of the decision.** "Scratchpad" makes "is this quick?" the
   natural question, and the measured answer was 1,150 chars against a cap of
   8,000.
6. **It renders last**, after the panel, and superseded copies collapse.
7. **The engine still picks no victims.** Decision 5 of this page stands
   unchanged.
8. **The trimmer at the wall is untouched.**
9. **A marked block, and a parse rule** that runs an unclosed span to the end of
   the reply. Over-capturing is repairable and a silent loss is not.

**Nothing about the objective moves.** Intelligence first, speed second, cost
third. The engine reads no signal about what the model is using, and the
selective-eviction family this page rejected in three forms stays rejected.

### What this changes in the code, none of it in this change

`MAX_SCRATCHPAD_CHARS` stops being a hard limit that errors and becomes a soft
threshold in the block's own header. The `scratchpad` tool goes, and `todo_write`
goes with it under the mode. The checklist moves into the same block, and two
write surfaces for one list is the cost bug twice over. `keep_in_context`
becomes `keep_open` and `ContextKept` follows it. A renamed `ScratchpadWritten`
owes a `Legacy alias:` line in `system-knowhow/thread-events.md`.

The mode is renamed **self-curated context mode**. Both glossaries, the
preference table and the event enumeration carry the new name already. The
preference key still spells it `context_mode_experimental`. ADRs 0085 to this
one keep the short name, because they record what was decided then.

### What would overturn the ten

Ten is provisional. It has three independent lines behind it, and the record
gives each: the usefulness instrument, the recovery distances, and JetBrains
tuning the same window to 10 turns on SWE-bench Verified. A benchmark run that
sweeps the window is what would move it. The three lines agree on the order of
magnitude and none of them pins the number.

## Amendment: the rename ships no alias, and no `Legacy alias:` line

The section above says a renamed `ScratchpadWritten` owes a `Legacy alias:` line
in `system-knowhow/thread-events.md`. It does not, and neither does
`ContextKept`. Both lines are deleted, and neither variant carries a
`serde(alias)` or a `LEGACY_TYPE_NAME_ALIASES` entry.

The mode has only ever run in the maintainer's own eval, on disposable
workspaces, so no workspace holds either row. A reader for rows that do not
exist is dead code, and the doc line would claim that reader exists.

The named cost: `workspace-audit.md`'s retired-event-name check reads that exact
string, so it cannot tell a dead `ScratchpadWritten` subscription from a
workspace domain event. `.claude/rules/system-knowhow.md` carries the
qualification, so the next rename knows when the line applies.
